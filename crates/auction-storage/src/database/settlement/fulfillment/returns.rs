//! 恢复流程不处理的结算退件的唯一消费方：只追加退件证据与消费事实、同事务建立退件案件，对应
//! 未终结履约转人工对账；消费本身不领取、不退回、不重发，也不写 `settlement_funding_returns` 或
//! 改变资金——这些只由案件经 Admin 处理后的持久动作执行。
//!
//! 退件来源的 SQL 判定 `system_settlement_return_source!` 与应用层
//! `system_returned_settlement_obligation_id` 同一口径。恢复流程只消费满足它且绑定恢复记录付款或
//! 交付义务的退件，这里消费其余全部退件，两边分区互斥，同一封邮件只消费一次。退件按证据请求的
//! 派发（首寄或正文派发引用指向的补寄）绑定派发历史中的原投递，不默认绑定最新投递。

use auction_application::settlement::{
    SettlementReturnConsumption, SettlementReturnDecision, SettlementReturnDelivery,
    SettlementReturnDispatchLookup, SettlementReturnDispatchRequest, SettlementReturnFacts,
    SettlementReturnMatch, SettlementReturnObligation, settlement_return_dispatch_request,
};
use auction_domain::FIRST_DISPATCH_NO;
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::map_storage_error;
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::repository_error;
use crate::database::settlement::returns::open_case;

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
    now: Timestamp,
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
    let metadata: serde_json::Value =
        candidate.try_get("return_metadata").map_err(map_storage_error)?;
    let request = settlement_return_dispatch_request(&metadata);
    let locked = match evidence_obligation {
        Some(obligation_id) => lock_obligation(&mut transaction, obligation_id, request).await?,
        None => None,
    };
    let obligation_id = locked.as_ref().map(|locked| locked.facts.id);
    let facts = SettlementReturnFacts {
        sender: candidate.try_get("return_sender").map_err(map_storage_error)?,
        subject: candidate.try_get("return_subject").map_err(map_storage_error)?,
        return_world: candidate.try_get("return_world_id").map_err(map_storage_error)?,
        received_at: candidate.try_get("return_received_at").map_err(map_storage_error)?,
        metadata,
        obligation: locked.as_ref().map(|locked| locked.facts.clone()),
    };
    let decision = facts.classify();
    let bound = matches!(
        decision.matched,
        SettlementReturnMatch::FulfillmentTerminal | SettlementReturnMatch::ManualReconcile
    );
    let bound_delivery = locked
        .as_ref()
        .filter(|_| bound)
        .and_then(|locked| locked.facts.delivery.as_ref())
        .map(|delivery| (delivery.id, delivery.dispatch_no));
    let delivery_id = bound_delivery.map(|(delivery_id, _)| delivery_id);
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
    let world_id: String = candidate.try_get("return_world_id").map_err(map_storage_error)?;
    // 案件以证据唯一：同事务建立，未绑定的记录同样建案隔离，等待核对或重新核验。
    open_case(
        &mut transaction,
        return_mail_id,
        &world_id,
        obligation_id,
        bound_delivery,
        decision,
        now,
    )
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

/// 已锁定的义务、履约与按证据请求解析的原派发；`state_version` 只在履约行存在时有值。
pub(crate) struct LockedObligation {
    pub(crate) facts: SettlementReturnObligation,
    pub(crate) state_version: Option<i64>,
}

/// 按义务 → 履约的既有锁序锁定行，再按证据请求解析原派发的发送事实。只认来源为该义务的结算
/// 投递；义务不存在时返回 `None`。
pub(crate) async fn lock_obligation(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    request: SettlementReturnDispatchRequest,
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
                  fulfillment.delivery_id, fulfillment.dispatch_no
           FROM public.settlement_obligation_fulfillments AS fulfillment
           WHERE fulfillment.obligation_id = $1
           FOR UPDATE OF fulfillment"#,
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let current_delivery: Option<Uuid> = fulfillment
        .as_ref()
        .map(|row| row.try_get::<Option<Uuid>, _>("delivery_id"))
        .transpose()
        .map_err(map_storage_error)?
        .flatten();
    let current_dispatch: Option<i32> = fulfillment
        .as_ref()
        .map(|row| row.try_get("dispatch_no"))
        .transpose()
        .map_err(map_storage_error)?;
    let (delivery, dispatch_lookup) =
        resolve_dispatch(transaction, obligation_id, request, current_delivery, current_dispatch)
            .await?;
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
            dispatch_lookup,
        },
        state_version: fulfillment
            .as_ref()
            .map(|row| row.try_get("state_version"))
            .transpose()
            .map_err(map_storage_error)?,
    }))
}

/// 按证据请求在只追加的派发历史中解析原派发：首寄查编号 1，补寄查引用的编号。迁移前已准备的
/// 首寄已补登历史；从未登记任何派发、履约仍是首寄时，沿用履约当前投递作为唯一首寄候选。
async fn resolve_dispatch(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    request: SettlementReturnDispatchRequest,
    current_delivery: Option<Uuid>,
    current_dispatch: Option<i32>,
) -> Result<
    (Option<SettlementReturnDelivery>, SettlementReturnDispatchLookup),
    auction_application::core::RepositoryError,
> {
    let dispatch_no = match request {
        SettlementReturnDispatchRequest::First => FIRST_DISPATCH_NO,
        SettlementReturnDispatchRequest::Resend(reference) => reference.dispatch_no,
        SettlementReturnDispatchRequest::Invalid => {
            return Ok((None, SettlementReturnDispatchLookup::Missing));
        }
    };
    let dispatch_no = i32::try_from(dispatch_no)
        .ok()
        .ok_or_else(|| repository_error("结算退件派发编号超出范围"))?;
    let recorded: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT delivery_id FROM public.settlement_fulfillment_dispatches
           WHERE obligation_id = $1 AND dispatch_no = $2"#,
    )
    .bind(obligation_id)
    .bind(dispatch_no)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let candidate = match recorded {
        Some(delivery_id) => Some(delivery_id),
        None if dispatch_no == 1 => {
            let any_recorded: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM public.settlement_fulfillment_dispatches
                                WHERE obligation_id = $1)",
            )
            .bind(obligation_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
            // 只登记了后续派发却缺首寄历史：无引用的退件不能默认绑定任何一次派发。
            if any_recorded {
                return Ok((None, SettlementReturnDispatchLookup::Ambiguous));
            }
            current_delivery.filter(|_| current_dispatch == Some(1))
        }
        None => None,
    };
    let Some(delivery_id) = candidate else {
        return Ok((None, SettlementReturnDispatchLookup::Missing));
    };
    let Some(row) = sqlx::query(
        r#"SELECT delivery.payload_world, delivery.payload_operation, delivery.sending_at,
                  delivery.recipient,
                  EXISTS (
                      SELECT 1 FROM public.settlement_return_cases AS bound
                      WHERE bound.delivery_id = delivery.id
                  ) AS already_returned,
                  COALESCE((
                      SELECT jsonb_agg(attempt.expectation -> 'body_digest'
                                       ORDER BY attempt.fencing_version)
                      FROM public.delivery_send_attempts AS attempt
                      WHERE attempt.delivery_id = delivery.id
                  ), '[]'::jsonb) AS body_digests
           FROM public.deliveries AS delivery
           WHERE delivery.id = $1
             AND delivery.origin_kind = 'settlement'
             AND delivery.origin_id = $2"#,
    )
    .bind(delivery_id)
    .bind(obligation_id.to_string())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    else {
        return Ok((None, SettlementReturnDispatchLookup::Missing));
    };
    let body_digests: Vec<[u8; 32]> = serde_json::from_value(
        row.try_get::<serde_json::Value, _>("body_digests").map_err(map_storage_error)?,
    )
    .ok()
    .ok_or_else(|| repository_error("结算退件原派发的发送摘要无效"))?;
    let delivery = SettlementReturnDelivery {
        id: delivery_id,
        world: row.try_get("payload_world").map_err(map_storage_error)?,
        operation: row.try_get("payload_operation").map_err(map_storage_error)?,
        sending_at: row.try_get("sending_at").map_err(map_storage_error)?,
        dispatch_no: u32::try_from(dispatch_no)
            .ok()
            .ok_or_else(|| repository_error("结算退件派发编号超出范围"))?,
        current: current_delivery == Some(delivery_id),
        recipient: row.try_get("recipient").map_err(map_storage_error)?,
        body_digests,
    };
    let lookup = if row.try_get::<bool, _>("already_returned").map_err(map_storage_error)? {
        SettlementReturnDispatchLookup::AlreadyReturned
    } else {
        SettlementReturnDispatchLookup::Unique
    };
    Ok((Some(delivery), lookup))
}

/// 退件证明收件人没有收下原邮件：未终结履约转人工对账并清除领取，原投递、证明与资金都不改写
/// （不变量 4、8）。版本与原状态不变才写入；持有旧 claim 的 Worker 随后按 fence 失败关闭。
pub(crate) async fn hold_for_manual_reconcile(
    transaction: &mut Transaction<'_, Postgres>,
    locked: &LockedObligation,
    return_mail_id: Uuid,
    decision: SettlementReturnDecision,
    now: Timestamp,
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
