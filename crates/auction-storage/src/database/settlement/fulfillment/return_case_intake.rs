//! 结算退件的同一事务写入链：锁定原义务与原派发、按应用层判据分类、追加唯一证据、建立案件，
//! 并在履约未终结时转人工对账。调用方拥有事务与提交权；批处理与隔离交还原案共用本入口。

use auction_application::settlement::{
    SettlementReturnConsumption, SettlementReturnDecision, SettlementReturnDelivery,
    SettlementReturnDispatchLookup, SettlementReturnDispatchRequest, SettlementReturnFacts,
    SettlementReturnMatch, SettlementReturnObligation, SettlementReturnReason,
    settlement_return_dispatch_request,
};
use auction_domain::FIRST_DISPATCH_NO;
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::map_storage_error;
use super::returns::{recovery_owned_settlement_return, system_settlement_return_source};
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::repository_error;
use crate::database::settlement::returns::open_case;

/// 退件转人工对账写入履约的稳定原因码。
const RETURNED_FAILURE_CODE: &str = "settlement_mail_returned";

/// 消费一封指定入站结算退件；调用方必须已经持有外层业务锁，并负责提交或回滚。
///
/// 不变量 2、3、6、8：证据、案件、履约迁移与追加审计在同一顶层事务写入；未知义务仍以
/// `unbound/obligation_unknown` 建案并返回空义务，重复调用只返回原证据的同一结果。
pub(crate) async fn consume_return_for_mail_in_tx(
    transaction: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    now: Timestamp,
) -> Result<SettlementReturnConsumption, auction_application::core::RepositoryError> {
    let Some(row) = sqlx::query(concat!(
        r#"SELECT inbound.world_id AS return_world_id,
                  inbound.received_at AS return_received_at,
                  inbound.mail_kind,
                  inbound.status,
                  inbound.decision_code,
                  inbound.decision_metadata AS return_metadata,
                  inbound.decision_metadata ->> 'obligation_id' AS evidence_obligation_id,
                  inbound.sender AS return_sender,
                  inbound.subject AS return_subject,
                  EXISTS (
                      SELECT 1 FROM public.settlement_recovery_return_consumptions AS consumed
                      WHERE consumed.return_mail_id = inbound.id
                  ) AS recovery_consumed,
                  EXISTS (
                      SELECT 1 FROM public.settlement_return_evidence AS evidence
                      WHERE evidence.return_mail_id = inbound.id
                  ) AS case_consumed,
                  EXISTS (
                      SELECT 1 FROM public.settlement_recovery_return_handoffs AS handoff
                      WHERE handoff.return_mail_id = inbound.id
                  ) AS recovery_handoff,
                  ("#,
        recovery_owned_settlement_return!(system_settlement_return_source!()),
        r#") AS recovery_owned
           FROM public.inbound_mail AS inbound
           WHERE inbound.id = $1"#
    ))
    .bind(return_mail_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    else {
        return Err(repository_error("结算退件引用的入站邮件不存在"));
    };
    let recovery_consumed: bool = row.try_get("recovery_consumed").map_err(map_storage_error)?;
    if recovery_consumed {
        return Err(repository_error("结算退件已由恢复流程消费"));
    }
    let recovery_handoff: bool = row.try_get("recovery_handoff").map_err(map_storage_error)?;
    if recovery_handoff {
        return Err(repository_error("结算退件已交给恢复流程"));
    }
    // 与恢复消费者共用同一分区谓词：系统 RTS 且绑定恢复付款/交割义务的退件只能由恢复消费。
    let recovery_owned: bool = row.try_get("recovery_owned").map_err(map_storage_error)?;
    if recovery_owned {
        return Err(repository_error("结算退件属于恢复流程"));
    }
    let case_consumed: bool = row.try_get("case_consumed").map_err(map_storage_error)?;
    if case_consumed {
        return existing_consumption(transaction, return_mail_id).await;
    }
    let mail_kind: String = row.try_get("mail_kind").map_err(map_storage_error)?;
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let decision_code: String = row.try_get("decision_code").map_err(map_storage_error)?;
    let return_received_at: Timestamp =
        row.try_get("return_received_at").map_err(map_storage_error)?;
    if mail_kind != "delivery"
        || status != "accepted"
        || decision_code != "settlement_recovery_returned"
        || return_received_at > now
    {
        return Err(repository_error("入站邮件不是可消费的结算退件"));
    }
    // 证据义务 ID 只按主键查找：损坏或不存在的引用都按“无义务”交给判定，不在 SQL 里强转报错。
    let evidence_obligation = row
        .try_get::<Option<String>, _>("evidence_obligation_id")
        .map_err(map_storage_error)?
        .and_then(|value| Uuid::parse_str(&value).ok());
    let metadata: serde_json::Value = row.try_get("return_metadata").map_err(map_storage_error)?;
    let request = settlement_return_dispatch_request(&metadata);
    let locked = match evidence_obligation {
        Some(obligation_id) => match lock_obligation(transaction, obligation_id, request).await? {
            ObligationLookup::Auction(locked) => Some(*locked),
            ObligationLookup::Missing => None,
            // 纵深防御：寄售义务不归拍卖 owner，不写证据、不建案件、不交接；邮件留在共享隔离。
            ObligationLookup::Consignment => {
                return Err(repository_error("结算退件绑定的是寄售义务，不属于拍卖"));
            }
        },
        None => None,
    };
    let obligation_id = locked.as_ref().map(|locked| locked.facts.id);
    let facts = SettlementReturnFacts {
        sender: row.try_get("return_sender").map_err(map_storage_error)?,
        subject: row.try_get("return_subject").map_err(map_storage_error)?,
        return_world: row.try_get("return_world_id").map_err(map_storage_error)?,
        received_at: return_received_at,
        metadata,
        obligation: locked.as_ref().map(|locked| locked.facts.clone()),
    };
    let decision = facts.classify();
    let bound_delivery = locked
        .as_ref()
        .filter(|_| decision.matched.bound())
        .and_then(|locked| locked.facts.delivery.as_ref())
        .map(|delivery| (delivery.id, delivery.dispatch_no));
    let delivery_id = bound_delivery.map(|(delivery_id, _)| delivery_id);
    let previous_state = locked
        .as_ref()
        .filter(|_| decision.matched.bound())
        .and_then(|locked| locked.facts.execution_state.clone());
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
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let world_id: String = row.try_get("return_world_id").map_err(map_storage_error)?;
    // 案件以证据唯一：同事务建立，未绑定的记录同样建案隔离，等待核对或重新核验。
    open_case(transaction, return_mail_id, &world_id, obligation_id, bound_delivery, decision, now)
        .await
        .map_err(map_storage_error)?;
    if decision.matched == SettlementReturnMatch::ManualReconcile {
        let locked = locked.as_ref().ok_or_else(|| repository_error("结算退件绑定的义务已失效"))?;
        hold_for_manual_reconcile(transaction, locked, return_mail_id, decision, now).await?;
    }
    Ok(SettlementReturnConsumption {
        return_mail_id,
        obligation_id,
        delivery_id,
        decision,
        recorded_at: now,
    })
}

/// 读取已消费退件的原证据；案件必须与证据同时存在，任何一方的损坏都失败关闭。
async fn existing_consumption(
    transaction: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
) -> Result<SettlementReturnConsumption, auction_application::core::RepositoryError> {
    let row = sqlx::query(
        "SELECT match_state, reason_code, obligation_id, delivery_id, recorded_at
         FROM public.settlement_return_evidence WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let case_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.settlement_return_cases WHERE return_mail_id = $1
         )",
    )
    .bind(return_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if !case_exists {
        return Err(repository_error("结算退件证据缺少原案件"));
    }
    let match_state: String = row.try_get("match_state").map_err(map_storage_error)?;
    let reason_code: String = row.try_get("reason_code").map_err(map_storage_error)?;
    let matched = SettlementReturnMatch::from_name(&match_state)
        .ok_or_else(|| repository_error("结算退件证据分类无效"))?;
    let reason = SettlementReturnReason::from_name(&reason_code)
        .ok_or_else(|| repository_error("结算退件证据原因无效"))?;
    Ok(SettlementReturnConsumption {
        return_mail_id,
        obligation_id: row.try_get("obligation_id").map_err(map_storage_error)?,
        delivery_id: row.try_get("delivery_id").map_err(map_storage_error)?,
        decision: SettlementReturnDecision { matched, reason },
        recorded_at: row.try_get("recorded_at").map_err(map_storage_error)?,
    })
}

/// 已锁定的义务、履约与按证据请求解析的原派发；`state_version` 只在履约行存在时有值。
pub(crate) struct LockedObligation {
    pub(crate) facts: SettlementReturnObligation,
    pub(crate) state_version: Option<i64>,
}

/// 义务查找的三态：拍卖义务、寄售义务（`contract_id` 非空，不属于本 owner）、不存在。
pub(crate) enum ObligationLookup {
    Auction(Box<LockedObligation>),
    Consignment,
    Missing,
}

impl ObligationLookup {
    /// 只取拍卖义务；寄售义务与不存在同样视为无义务，供不建案件的核验路径使用。
    pub(crate) fn into_auction(self) -> Option<LockedObligation> {
        match self {
            Self::Auction(locked) => Some(*locked),
            Self::Consignment | Self::Missing => None,
        }
    }
}

/// 按义务 → 履约的既有锁序锁定行，再按证据请求解析原派发的发送事实。只认来源为该义务的结算
/// 投递；寄售义务单独返回 `Consignment`，不会被拍卖退件案件绑定、建案或改写履约（不变量 6）。
pub(crate) async fn lock_obligation(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    request: SettlementReturnDispatchRequest,
) -> Result<ObligationLookup, auction_application::core::RepositoryError> {
    let Some(obligation) = sqlx::query(
        r#"SELECT obligation.obligation_kind, obligation.amount, obligation.carrier_credit,
                  obligation.contract_id IS NOT NULL AS consignment,
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
        return Ok(ObligationLookup::Missing);
    };
    if obligation.try_get::<bool, _>("consignment").map_err(map_storage_error)? {
        return Ok(ObligationLookup::Consignment);
    }
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
    Ok(ObligationLookup::Auction(Box::new(LockedObligation {
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
    })))
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
