//! 恢复接管退件之后被投递门禁留在待发的原结算投递：恢复 owner 收口时同事务终结。
//!
//! 退件到达之前原付款投递可能已被重新排队（结果未知后的对账重试等）；恢复流程接管退件后，领取与
//! 发送围栏的接管谓词让它不会再被领取或发送，Admin 也不能释放或重试，原来只能长期停在
//! pending。它当前 fence 从未进入发送围栏（`sending_at` 为空），所以是确定没有外写的意图：收回、
//! 取消或自动退回卖家收口时，恢复 owner 在同一事务把它终结为 `failed/recovery_superseded`，结果
//! 元数据明确记录 `write_started=false` 与 `outcome_unknown=false`，推进 fence 让任何旧 claim 失效，
//! 并追加 `delivery.fail` 审计；随后收口事务在新 fence 上追加的 `cod_returned` 处置（来源为本恢复
//! 记录）就是它的终结事实。既有发送尝试、Sent 绑定与审计都不改写（不变量 2、4、8）。

use auction_application::recovery::RecoveryCommandError;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::audit::{DeliveryAudit, FAIL, append as append_delivery_audit};
use crate::database::recovery::commands::mapping::map_sql_error;

/// 被恢复接管终结的原投递失败码。
pub(super) const RECOVERY_SUPERSEDED_CODE: &str = "recovery_superseded";

/// 终结操作的审计身份（Admin 确认或 Worker 自动推进）。
pub(super) struct SupersedingActor<'a> {
    pub(super) actor_type: &'static str,
    pub(super) actor_id: &'a str,
    pub(super) source: &'static str,
}

/// 终结义务履约当前派发上从未进入发送围栏的待发投递；没有这样的投递时返回 `None`。调用方已按
/// 既有锁序锁定恢复记录与义务（确认事务另已锁定该投递行），并在同一事务内随后追加收口处置。
pub(super) async fn retire_superseded_dispatch(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    recovery_id: Uuid,
    actor: &SupersedingActor<'_>,
    now: DateTime<Utc>,
) -> Result<Option<Uuid>, RecoveryCommandError> {
    let Some(row) = sqlx::query(
        r#"UPDATE public.deliveries AS delivery
           SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
               completed_at = GREATEST($3, delivery.created_at, delivery.claimed_at),
               external_reference = NULL, failure_code = $4,
               result_metadata = jsonb_build_object(
                   'write_started', false, 'outcome_unknown', false,
                   'recovery_superseded', true, 'settlement_recovery_id', $2::text),
               fencing_version = delivery.fencing_version + 1,
               updated_at = GREATEST(CURRENT_TIMESTAMP, $3, delivery.created_at,
                                     delivery.claimed_at)
           WHERE delivery.id = (
                   SELECT fulfillment.delivery_id
                   FROM public.settlement_obligation_fulfillments AS fulfillment
                   WHERE fulfillment.obligation_id = $1)
             AND delivery.origin_kind = 'settlement'
             AND delivery.origin_id = $1::text
             AND delivery.status IN ('pending', 'claimed')
             AND delivery.sending_at IS NULL
           RETURNING delivery.id, delivery.command_id, delivery.fencing_version,
                     delivery.attempt_count"#,
    )
    .bind(obligation_id)
    .bind(recovery_id)
    .bind(now)
    .bind(RECOVERY_SUPERSEDED_CODE)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    else {
        return Ok(None);
    };
    let delivery_id: Uuid = row.try_get("id").map_err(map_sql_error)?;
    let command_id: Uuid = row.try_get("command_id").map_err(map_sql_error)?;
    let fencing_version: i64 = row.try_get("fencing_version").map_err(map_sql_error)?;
    let attempt_count: i32 = row.try_get("attempt_count").map_err(map_sql_error)?;
    let request_id = command_id.to_string();
    append_delivery_audit(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            actor_type: actor.actor_type,
            actor_id: actor.actor_id,
            action: FAIL,
            delivery_id,
            request_id: Some(&request_id),
            source: actor.source,
            metadata: serde_json::json!({
                "status": "failed",
                "failure_code": RECOVERY_SUPERSEDED_CODE,
                "fencing_version": fencing_version,
                "attempt_count": attempt_count,
                "settlement_recovery_id": recovery_id,
                "obligation_id": obligation_id,
            }),
        },
    )
    .await
    .map_err(map_sql_error)?;
    Ok(Some(delivery_id))
}
