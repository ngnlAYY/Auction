//! 恢复流程不处理的结算退件的唯一消费方：只追加退件证据与消费事实，对应未终结履约同事务转
//! 人工对账；不领取、不退回、不重发，也不写 `settlement_funding_returns` 或改变资金。
//!
//! 退件来源的 SQL 判定 `system_settlement_return_source!` 与应用层
//! `system_returned_settlement_obligation_id` 同一口径。恢复流程只消费满足它且绑定恢复记录付款或
//! 交付义务的退件，这里消费其余全部退件，两边分区互斥，同一封邮件只消费一次。

use auction_application::settlement::{
    SettlementReturnConsumption, SettlementReturnDecision, SettlementReturnDelivery,
    SettlementReturnFacts, SettlementReturnMatch, SettlementReturnObligation,
};
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::{map_storage_error, repository_error};
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 退件转人工对账写入履约的稳定原因码。
const RETURNED_FAILURE_CODE: &str = "settlement_mail_returned";

/// 入站退件来源可证明的 SQL 谓词（别名 `inbound` 为 `public.inbound_mail`）：持久化的发件人恰为
/// 系统 `MoogleMail`，主题按 ASCII 不区分大小写等于 `[RTS] ` + 证据义务 ID 的 canonical 结算主题。
/// 缺少发件人或主题的历史行为假，不会因 NULL 落入两个分区之外。
macro_rules! system_settlement_return_source {
    () => {
        r#"COALESCE(
               inbound.sender = 'MoogleMail'
               AND translate(inbound.subject, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ',
                             'abcdefghijklmnopqrstuvwxyz') COLLATE "C" IN (
                   '[rts] auction equipment delivery ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction payment request ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction seller payout ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']',
                   '[rts] auction buyer refund ['
                       || (inbound.decision_metadata ->> 'obligation_id') || ']'),
               false)"#
    };
}
pub(crate) use system_settlement_return_source;

/// 消费一封尚未消费的结算退件；没有时返回 `None`。单次 Serializable 事务，失败由 Worker 下一轮
/// 重试：无论证据是否可信或可绑定都会消费，不留下每轮重选的队头。
pub(crate) async fn consume_return_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<SettlementReturnConsumption>, auction_application::core::RepositoryError> {
    let mut transaction =
        pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.map_err(map_storage_error)?;
    let Some(candidate) = sqlx::query(concat!(
        r#"SELECT inbound.id AS return_mail_id,
                  inbound.world_id AS return_world_id,
                  inbound.received_at AS return_received_at,
                  inbound.decision_metadata AS return_metadata,
                  inbound.decision_metadata ->> 'obligation_id' AS evidence_obligation_id,
                  inbound.sender AS return_sender,
                  inbound.subject AS return_subject
           FROM public.inbound_mail AS inbound
           WHERE inbound.mail_kind = 'delivery'
             AND inbound.status = 'accepted'
             AND inbound.decision_code = 'settlement_recovery_returned'
             AND inbound.received_at <= $1
             AND NOT EXISTS (
                 SELECT 1 FROM public.settlement_recovery_return_consumptions AS consumed
                 WHERE consumed.return_mail_id = inbound.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM public.settlement_return_evidence AS evidence
                 WHERE evidence.return_mail_id = inbound.id
             )
             AND NOT ("#,
        system_settlement_return_source!(),
        r#" AND EXISTS (
                 SELECT 1 FROM public.settlement_recoveries AS recovery
                 WHERE inbound.decision_metadata ->> 'obligation_id' IN (
                     recovery.payment_obligation_id::text,
                     recovery.equipment_obligation_id::text
                 )
             ))
           ORDER BY inbound.received_at, inbound.id
           LIMIT 1"#
    ))
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    else {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(None);
    };
    let return_mail_id: Uuid = candidate.try_get("return_mail_id").map_err(map_storage_error)?;
    // 证据义务 ID 只按主键查找：损坏或不存在的引用都按“无义务”交给判定，不在 SQL 里强转报错。
    let evidence_obligation = candidate
        .try_get::<Option<String>, _>("evidence_obligation_id")
        .map_err(map_storage_error)?
        .and_then(|value| Uuid::parse_str(&value).ok());
    let locked = match evidence_obligation {
        Some(obligation_id) => lock_obligation(&mut transaction, obligation_id).await?,
        None => None,
    };
    let obligation_id = locked.as_ref().map(|locked| locked.facts.id);
    let facts = SettlementReturnFacts {
        sender: candidate.try_get("return_sender").map_err(map_storage_error)?,
        subject: candidate.try_get("return_subject").map_err(map_storage_error)?,
        return_world: candidate.try_get("return_world_id").map_err(map_storage_error)?,
        received_at: candidate.try_get("return_received_at").map_err(map_storage_error)?,
        metadata: candidate.try_get("return_metadata").map_err(map_storage_error)?,
        obligation: locked.as_ref().map(|locked| locked.facts.clone()),
    };
    let decision = facts.classify();
    let bound = matches!(
        decision.matched,
        SettlementReturnMatch::FulfillmentTerminal | SettlementReturnMatch::ManualReconcile
    );
    let delivery_id = locked
        .as_ref()
        .filter(|_| bound)
        .and_then(|locked| locked.facts.delivery.as_ref())
        .map(|delivery| delivery.id);
    let previous_state =
        locked.as_ref().filter(|_| bound).and_then(|locked| locked.facts.execution_state.clone());
    // 消费唯一事实先于任何状态迁移写入：主键冲突时整事务回滚，不会出现两次转人工。
    sqlx::query(
        r#"INSERT INTO public.settlement_return_evidence
               (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
                previous_execution_state, recorded_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(return_mail_id)
    .bind(decision.matched.as_str())
    .bind(decision.reason.as_str())
    .bind(obligation_id)
    .bind(delivery_id)
    .bind(previous_state.as_deref())
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    if decision.matched == SettlementReturnMatch::ManualReconcile {
        let locked = locked.as_ref().ok_or_else(|| repository_error("结算退件绑定的义务已失效"))?;
        hold_for_manual_reconcile(&mut transaction, locked, return_mail_id, decision, now).await?;
    }
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(Some(SettlementReturnConsumption {
        return_mail_id,
        obligation_id,
        delivery_id,
        decision,
        recorded_at: now,
    }))
}

/// 已锁定的义务、履约与原投递事实；`state_version` 只在履约行存在时有值。
struct LockedObligation {
    facts: SettlementReturnObligation,
    state_version: Option<i64>,
}

/// 按义务 → 履约的既有锁序锁定行，再读取原出站投递的发送事实。只认来源为该义务的结算投递；
/// 义务不存在时返回 `None`。
async fn lock_obligation(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<Option<LockedObligation>, auction_application::core::RepositoryError> {
    let Some(obligation) = sqlx::query(
        r#"SELECT obligation.obligation_kind, obligation.amount, obligation.carrier_credit,
                  equipment.external_equipment_id
           FROM public.settlement_obligations AS obligation
           LEFT JOIN public.equipment AS equipment ON equipment.id = obligation.equipment_id
           WHERE obligation.id = $1
           FOR UPDATE OF obligation"#,
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    else {
        return Ok(None);
    };
    let fulfillment = sqlx::query(
        r#"SELECT fulfillment.execution_state, fulfillment.state_version,
                  delivery.id AS delivery_id, delivery.payload_world, delivery.payload_operation,
                  delivery.sending_at
           FROM public.settlement_obligation_fulfillments AS fulfillment
           LEFT JOIN public.deliveries AS delivery
               ON delivery.id = fulfillment.delivery_id
              AND delivery.origin_kind = 'settlement'
              AND delivery.origin_id = fulfillment.obligation_id::text
           WHERE fulfillment.obligation_id = $1
           FOR UPDATE OF fulfillment"#,
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let delivery = fulfillment.as_ref().map(map_delivery).transpose()?.flatten();
    Ok(Some(LockedObligation {
        facts: SettlementReturnObligation {
            id: obligation_id,
            kind: obligation.try_get("obligation_kind").map_err(map_storage_error)?,
            amount: obligation.try_get("amount").map_err(map_storage_error)?,
            carrier_credit: obligation.try_get("carrier_credit").map_err(map_storage_error)?,
            external_equipment_id: obligation
                .try_get("external_equipment_id")
                .map_err(map_storage_error)?,
            execution_state: fulfillment
                .as_ref()
                .map(|row| row.try_get("execution_state"))
                .transpose()
                .map_err(map_storage_error)?,
            delivery,
        },
        state_version: fulfillment
            .as_ref()
            .map(|row| row.try_get("state_version"))
            .transpose()
            .map_err(map_storage_error)?,
    }))
}

fn map_delivery(
    row: &PgRow,
) -> Result<Option<SettlementReturnDelivery>, auction_application::core::RepositoryError> {
    let Some(id) = row.try_get::<Option<Uuid>, _>("delivery_id").map_err(map_storage_error)? else {
        return Ok(None);
    };
    Ok(Some(SettlementReturnDelivery {
        id,
        world: row.try_get("payload_world").map_err(map_storage_error)?,
        operation: row.try_get("payload_operation").map_err(map_storage_error)?,
        sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
    }))
}

/// 退件证明收件人没有收下原邮件：未终结履约转人工对账并清除领取，原投递、证明与资金都不改写
/// （不变量 4、8）。版本与原状态不变才写入；持有旧 claim 的 Worker 随后按 fence 失败关闭。
async fn hold_for_manual_reconcile(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedObligation,
    return_mail_id: Uuid,
    decision: SettlementReturnDecision,
    now: DateTime<Utc>,
) -> Result<(), auction_application::core::RepositoryError> {
    let state = locked.facts.execution_state.as_deref();
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'manual_reconcile',
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               last_error_code = $4,
               completed_at = $5,
               state_version = state_version + 1,
               updated_at = $5
           WHERE obligation_id = $1
             AND execution_state = $2
             AND state_version = $3
             AND execution_state IN ('pending', 'prepared', 'enqueued')"#,
    )
    .bind(locked.facts.id)
    .bind(state)
    .bind(locked.state_version)
    .bind(RETURNED_FAILURE_CODE)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if updated.rows_affected() != 1 {
        return Err(repository_error("结算退件对应的履约状态已变化"));
    }
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, 'system', 'auction-fulfillment',
                   'auction.fulfillment.manual_reconcile',
                   'settlement_obligation', $3, 'success', 'worker', $4)"#,
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(locked.facts.id.to_string())
    .bind(serde_json::json!({
        "state": "manual_reconcile",
        "previous_state": state,
        "error_code": RETURNED_FAILURE_CODE,
        "return_mail_id": return_mail_id,
        "return_reason": decision.reason.as_str(),
        "delivery_id": locked.facts.delivery.as_ref().map(|delivery| delivery.id),
    }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}
