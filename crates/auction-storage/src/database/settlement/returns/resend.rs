//! 原义务上的补寄与履约收口：授权补寄把履约重置为下一派发编号的 pending，交给既有履约 Worker
//! 按新身份准备、激活与核对；代管或留箱结案以明确原因终结执行中的履约；补寄的可信寄出证明
//! 成立后结束本案。原投递、原发送证明与原领回事实都不改写（不变量 8）。

use auction_application::core::ApplicationError;
use auction_application::settlement::{SettlementReturnError, SettlementReturnProgress};
use auction_domain::{SettlementReturnCaseStatus, SettlementReturnPlan};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Postgres;
use uuid::Uuid;

use super::admission::FulfillmentRow;
use super::cases::{CaseEvent, CaseRow, CaseTransition, load_case, transition};
use super::errors::ReturnTransactionError;
use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 授权一次补寄并返回新派发编号。只接受本案派发仍是当前派发、且履约停在人工对账（退件消费）
/// 或本案代管终结的履约；版本比较交换失败按状态冲突拒绝，不覆盖并发推进。
pub(super) async fn authorize_resend(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    fulfillment: &FulfillmentRow,
    now: DateTime<Utc>,
) -> Result<i32, ReturnTransactionError> {
    let (Some(obligation_id), Some(delivery_id)) = (case.obligation_id, case.delivery_id) else {
        return Err(SettlementReturnError::ObligationIneligible.into());
    };
    let next = fulfillment.dispatch_no.checked_add(1).ok_or(SettlementReturnError::ResendLimit)?;
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'pending',
               dispatch_no = $5,
               delivery_id = NULL,
               prepare_command_id = NULL,
               prepare_request_digest = NULL,
               activate_command_id = NULL,
               activate_request_digest = NULL,
               proof_kind = NULL,
               proof_reference = NULL,
               proof_digest = NULL,
               proof_observed_at = NULL,
               last_error_code = NULL,
               completed_at = NULL,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               retry_count = 0,
               state_version = state_version + 1,
               updated_at = $6
           WHERE obligation_id = $1
             AND state_version = $2
             AND delivery_id = $3
             AND dispatch_no = $4
             AND execution_state IN ('manual_reconcile', 'failed')"#,
    )
    .bind(obligation_id)
    .bind(fulfillment.state_version)
    .bind(delivery_id)
    .bind(fulfillment.dispatch_no)
    .bind(next)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(SettlementReturnError::ActionConflict.into());
    }
    fulfillment_audit(
        tx,
        obligation_id,
        "auction.fulfillment.resend_authorized",
        json!({
            "state": "pending",
            "previous_state": fulfillment.execution_state,
            "return_mail_id": case.return_mail_id,
            "previous_delivery_id": delivery_id,
            "dispatch_no": next,
        }),
        now,
    )
    .await?;
    Ok(next)
}

/// 代管或留箱结案：本案派发仍是当前派发且履约停在人工对账时，以明确原因改为 failed；权益不因
/// 此消灭，原资金用途继续受限。履约已不是本案派发或不在人工对账时不改写。
pub(super) async fn close_fulfillment(
    tx: &mut Transaction<'_, Postgres>,
    case: &CaseRow,
    fulfillment: Option<&FulfillmentRow>,
    failure_code: &'static str,
    now: DateTime<Utc>,
) -> Result<(), ReturnTransactionError> {
    let (Some(obligation_id), Some(delivery_id), Some(fulfillment)) =
        (case.obligation_id, case.delivery_id, fulfillment)
    else {
        return Ok(());
    };
    if fulfillment.execution_state != "manual_reconcile"
        || fulfillment.delivery_id != Some(delivery_id)
    {
        return Ok(());
    }
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'failed',
               last_error_code = $4,
               state_version = state_version + 1,
               updated_at = $5
           WHERE obligation_id = $1
             AND state_version = $2
             AND delivery_id = $3
             AND execution_state = 'manual_reconcile'"#,
    )
    .bind(obligation_id)
    .bind(fulfillment.state_version)
    .bind(delivery_id)
    .bind(failure_code)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(SettlementReturnError::ActionConflict.into());
    }
    fulfillment_audit(
        tx,
        obligation_id,
        "auction.fulfillment.failed",
        json!({
            "state": "failed",
            "previous_state": "manual_reconcile",
            "error_code": failure_code,
            "return_mail_id": case.return_mail_id,
            "delivery_id": delivery_id,
        }),
        now,
    )
    .await
}

async fn fulfillment_audit(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    action: &'static str,
    metadata: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<(), ReturnTransactionError> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, 'system', 'auction-fulfillment', $3,
                   'settlement_obligation', $4, 'success', 'worker', $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(action)
    .bind(obligation_id.to_string())
    .bind(metadata)
    .execute(&mut **tx)
    .await
    .record_audit_failure()?;
    Ok(())
}

/// 收口一项补寄已有可信寄出证明的案件：派发历史中本案授权的投递已成功且有 Sent 证明。
pub(super) async fn complete_due_resend_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<SettlementReturnProgress>, ApplicationError> {
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::Fulfillment,
        crate::database::SERIALIZABLE_ATTEMPTS,
        ReturnTransactionError::retryable,
        || complete_once(pool, now),
    )
    .await
    .map_err(ReturnTransactionError::into_application)
}

async fn complete_once(
    pool: &crate::database::TrackedPool<Postgres>,
    now: DateTime<Utc>,
) -> Result<Option<SettlementReturnProgress>, ReturnTransactionError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    let candidate: Option<(Uuid, Uuid, i32)> = sqlx::query_as(
        r#"SELECT c.return_mail_id, dispatch.delivery_id, dispatch.dispatch_no
           FROM public.settlement_return_cases AS c
           JOIN public.settlement_fulfillment_dispatches AS dispatch
             ON dispatch.obligation_id = c.obligation_id
            AND dispatch.dispatch_no = c.resend_dispatch_no
            AND dispatch.authorized_by = c.return_mail_id
           JOIN public.deliveries AS delivery ON delivery.id = dispatch.delivery_id
           WHERE c.status = 'resend_pending'
             AND c.closed_at IS NULL
             AND delivery.status = 'succeeded'
             AND delivery.result_metadata ->> 'external_reference_proven' = 'true'
             AND (delivery.result_metadata ->> 'proof' = 'sent_exact_payload_v1'
                  OR COALESCE(public.delivery_has_frozen_sent_proof(
                      delivery.id, delivery.result_metadata, delivery.external_reference
                  ), false))
           ORDER BY c.updated_at, c.return_mail_id
           LIMIT 1"#,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((return_mail_id, delivery_id, dispatch_no)) = candidate else {
        tx.commit().await?;
        return Ok(None);
    };
    let Some(case) = load_case(&mut tx, return_mail_id, true).await? else {
        tx.commit().await?;
        return Ok(None);
    };
    if case.status != SettlementReturnCaseStatus::ResendPending
        || case.resend_dispatch_no != Some(dispatch_no)
    {
        tx.commit().await?;
        return Ok(None);
    }
    let mut next = CaseTransition::from_case(&case, SettlementReturnCaseStatus::Resent);
    next.plan = Some(SettlementReturnPlan::Resend);
    let version = transition(
        &mut tx,
        &case,
        &next,
        CaseEvent {
            kind: "resent",
            command: None,
            details: json!({"dispatch_no": dispatch_no, "delivery_id": delivery_id}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(SettlementReturnProgress {
        return_mail_id,
        status: SettlementReturnCaseStatus::Resent,
        case_version: version,
        event_kind: "resent",
    }))
}
