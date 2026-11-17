//! Admin 确认成交撤销（`reclaimed`/`cancelled`）时，在确认事务内收口原付款履约与不再可执行的
//! 依赖履约，并在原付款投递上追加 `cod_returned` 统一处置事实。
//!
//! 只复用既有 `failed` 执行状态与明确失败码，不新增状态；原投递、发送证明与资金事实保持不变。
//! 原付款已满足、原付款投递仍可能外发、已消费的退件不能把原付款每一次可能外达的发送逐一归属
//! （含当前退件之后又有发送），或依赖履约已开始执行时拒绝确认，交给人工核对。

use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::disposition::{DeliveryDispositionFact, append};
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::recovery::commands::binding::return_attributable_to_sends;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::RecoveryRow;

/// 依赖原付款、随成交撤销一起终结的卖方货款与 isekai 装备交付履约的失败码。
pub(super) const SALE_VOIDED_CODE: &str = "recovery_sale_voided";

/// 收口原付款与依赖履约；`equipment_recovered` 不撤销付款（买方已付款，退款继续执行），其依赖
/// 履约的收口见 `recovered`。
pub(in crate::database::recovery::commands) async fn close_voided_fulfillments(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    command: &RecoveryConfirmationCommand,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let payment_code = match command.action {
        RecoveryConfirmationAction::Reclaimed => "recovery_reclaimed",
        RecoveryConfirmationAction::Cancelled => "recovery_cancelled",
        RecoveryConfirmationAction::EquipmentRecovered => return Ok(()),
    };
    // 原付款投递已在履约行之前按规范锁序锁定，履约行随后由 validate_obligations 按 UUID 序锁定；
    // 这里只读状态与 fence。
    let payment = sqlx::query(
        r#"SELECT execution_state, delivery_id
           FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = $1"#,
    )
    .bind(recovery.payment_obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let mut payment_delivery = None;
    let mut payment_state = None;
    if let Some(row) = payment {
        let state: String = row.try_get("execution_state").map_err(map_sql_error)?;
        // 已满足说明买方已付款，成交不能再撤销。
        if state == "satisfied" {
            return Err(RecoveryCommandError::NotConfirmable);
        }
        if let Some(delivery_id) =
            row.try_get::<Option<Uuid>, _>("delivery_id").map_err(map_sql_error)?
        {
            let delivery = sqlx::query(
                "SELECT status, fencing_version FROM public.deliveries WHERE id = $1 FOR UPDATE",
            )
            .bind(delivery_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_sql_error)?;
            let status: String = delivery.try_get("status").map_err(map_sql_error)?;
            // 投递仍在准备、排队或发送中：外部写入尚未解决，不能自动收口。
            if matches!(status.as_str(), "prepared" | "pending" | "claimed" | "sending") {
                return Err(RecoveryCommandError::NotConfirmable);
            }
            let fencing: i64 = delivery.try_get("fencing_version").map_err(map_sql_error)?;
            payment_delivery = Some((delivery_id, fencing));
        }
        payment_state = Some(state);
    }
    // 退件与 Take 只证明某一次发送被收回：原付款义务每一次可能已到达远端的发送都必须由本恢复
    // 记录已消费的系统退件逐一归属，且当前退件之后没有任何发送。否则收口会与仍可能在买家手里的
    // 邮件冲突，处置也不能落在原投递的 fence 上；返回明确的阻断原因交人工（不变量 2、4、6）。
    if !return_attributable_to_sends(transaction, recovery.payment_obligation_id, recovery.id)
        .await?
    {
        return Err(RecoveryCommandError::SendUnattributed);
    }

    let mut dependents = vec![recovery.seller_payout_obligation_id];
    dependents.extend(recovery.equipment_obligation_id);
    let rows = sqlx::query(
        r#"SELECT obligation_id, execution_state,
                  delivery_id IS NULL AND prepare_command_id IS NULL AS untouched
           FROM public.settlement_obligation_fulfillments
           WHERE obligation_id = ANY($1)
           ORDER BY obligation_id"#,
    )
    .bind(&dependents)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let mut open_dependents = Vec::new();
    for row in rows {
        let state: String = row.try_get("execution_state").map_err(map_sql_error)?;
        let untouched: bool = row.try_get("untouched").map_err(map_sql_error)?;
        match (state.as_str(), untouched) {
            // 已经失败的依赖保持原失败码，已追加的事实不改写。
            ("failed", _) => {}
            ("pending", true) => {
                open_dependents
                    .push(row.try_get::<Uuid, _>("obligation_id").map_err(map_sql_error)?);
            }
            // 依赖已开始执行或已有结论：撤销会与之冲突，交给人工核对。
            _ => return Err(RecoveryCommandError::NotConfirmable),
        }
    }

    let mut closures = Vec::with_capacity(open_dependents.len() + 1);
    if let Some(state) = payment_state.filter(|state| {
        matches!(state.as_str(), "pending" | "prepared" | "enqueued" | "manual_reconcile")
    }) {
        closures.push((recovery.payment_obligation_id, state, payment_code));
    }
    closures
        .extend(open_dependents.into_iter().map(|id| (id, "pending".to_owned(), SALE_VOIDED_CODE)));
    for (obligation_id, previous_state, failure_code) in closures {
        fail_fulfillment(transaction, obligation_id, failure_code, now).await?;
        append_closure_audit(
            transaction,
            ClosureAudit {
                obligation_id,
                recovery_id: recovery.id,
                previous_state: &previous_state,
                failure_code,
            },
            command,
            now,
        )
        .await?;
    }

    // 处置事实绑定原付款投递的当前 fence，来源引用恢复记录；读侧据此解除这些失败义务的告警。
    if let Some((delivery_id, fencing_version)) = payment_delivery.filter(|(_, fence)| *fence >= 1)
    {
        let reason = format!(
            "Admin confirmed {} after {} COD returns (limit {})",
            command.action.as_str(),
            recovery.cod_return_count,
            recovery.cod_return_limit
        );
        let actor_id = if auction_domain::valid_text(&command.actor.actor_id, 128) {
            command.actor.actor_id.as_str()
        } else {
            "admin"
        };
        append(
            transaction,
            DeliveryDispositionFact {
                delivery_id,
                fencing_version,
                kind: auction_domain::DeliveryDispositionKind::CodReturned,
                source: auction_domain::DeliveryDispositionSource {
                    settlement_recovery_id: Some(recovery.id),
                    ..Default::default()
                },
                actor_source: auction_domain::DeliveryDispositionActorSource::Admin,
                actor_id,
                reason: Some(&reason),
                reason_code: Some(payment_code),
                detail: Some(&serde_json::json!({
                    "recovery_id": recovery.id,
                    "command_id": command.command_id.as_uuid(),
                    "payment_obligation_id": recovery.payment_obligation_id,
                    "seller_payout_obligation_id": recovery.seller_payout_obligation_id,
                    "equipment_obligation_id": recovery.equipment_obligation_id,
                })),
                command_id: None,
                request_digest: None,
            },
        )
        .await
        .map_err(map_sql_error)?;
    }
    Ok(())
}

/// 把一条未终结履约按业务取消收口为 `failed`，清除领取与重试时间；持有旧 claim 的 Worker
/// 随后按版本与 fence 失败关闭。
pub(super) async fn fail_fulfillment(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    failure_code: &str,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'failed',
               last_error_code = $2,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               next_attempt_at = NULL,
               proof_kind = NULL,
               proof_reference = NULL,
               proof_digest = NULL,
               proof_observed_at = NULL,
               completed_at = $3,
               state_version = state_version + 1,
               updated_at = $3
           WHERE obligation_id = $1
             AND execution_state IN ('pending', 'prepared', 'enqueued', 'manual_reconcile')"#,
    )
    .bind(obligation_id)
    .bind(failure_code)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.rows_affected() != 1 {
        return Err(RecoveryCommandError::StaleState);
    }
    Ok(())
}

/// 一条被收口履约的审计要素。
pub(super) struct ClosureAudit<'a> {
    pub(super) obligation_id: Uuid,
    pub(super) recovery_id: Uuid,
    pub(super) previous_state: &'a str,
    pub(super) failure_code: &'a str,
}

/// 与收口同事务追加履约终态审计，操作者为确认命令的 Admin（不变量 8）。
pub(super) async fn append_closure_audit(
    transaction: &mut Transaction<'_, Postgres>,
    audit: ClosureAudit<'_>,
    command: &RecoveryConfirmationCommand,
    now: DateTime<Utc>,
) -> Result<(), RecoveryCommandError> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, source, metadata)
           VALUES ($1, $2, $3, $4, 'auction.fulfillment.failed', 'settlement_obligation', $5,
                   'success', 'admin', $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(command.actor.actor_type.database_name())
    .bind(&command.actor.actor_id)
    .bind(audit.obligation_id.to_string())
    .bind(serde_json::json!({
        "state": "failed",
        "previous_state": audit.previous_state,
        "error_code": audit.failure_code,
        "recovery_id": audit.recovery_id,
        "command_id": command.command_id.as_uuid(),
        "resolution": command.action.as_str(),
    }))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}
