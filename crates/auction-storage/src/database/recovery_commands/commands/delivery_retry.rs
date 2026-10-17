//! Delivery Admin 重试与 Recovery 父子投影的原子衔接。

use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

#[path = "delivery_retry/locks.rs"]
mod locks;
pub(crate) use locks::{lock_recovery_delivery_retry_actions, lock_recovery_delivery_retry_scope};

/// 调用方须先按统一序调用scope→中间业务锁→actions，再锁delivery、复核版本与人工授权。
/// 两阶段前缀和本函数必须在同一Serializable事务；这里只更新已预锁行并追加原命令/审计。
/// 这里只恢复失败投影，不生成成功证明、不重新准备资产动作，也不清除其它 action 的阻断。
pub(crate) async fn reset_after_delivery_retry(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT action.id, action.recovery_id, action.execution_state,
                action.last_error_code, recovery.blocking_code,
                recovery.manual_reconcile_delivery_id, recovery.stage, recovery.resolution
         FROM public.settlement_recovery_actions AS action
         INNER JOIN public.settlement_recoveries AS recovery ON recovery.id = action.recovery_id
         WHERE action.delivery_id = $1
         ORDER BY action.id",
    )
    .bind(delivery_id)
    .fetch_all(&mut **transaction)
    .await?;
    let Some(row) = rows.first() else { return Ok(true) };
    if rows.len() != 1
        || row.try_get::<String, _>("stage")? == "completed"
        || row.try_get::<Option<String>, _>("resolution")?.is_some()
    {
        return Ok(false);
    }
    let state: String = row.try_get("execution_state")?;
    let blocker: Option<String> = row.try_get("blocking_code")?;
    let manual_delivery: Option<Uuid> = row.try_get("manual_reconcile_delivery_id")?;
    if state == "enqueued" {
        // Worker 尚未投影 delivery 失败时，无需重写仍有效的 action claim。
        return Ok(blocker.is_none() && manual_delivery.is_none());
    }
    let expected_error = match state.as_str() {
        "failed" if manual_delivery.is_none() => "recovery_delivery_failed",
        "manual_reconcile" if manual_delivery == Some(delivery_id) => "recovery_outcome_unknown",
        _ => return Ok(false),
    };
    if blocker.as_deref() != Some(expected_error)
        || row.try_get::<Option<String>, _>("last_error_code")?.as_deref() != Some(expected_error)
    {
        return Ok(false);
    }
    let action_id: Uuid = row.try_get("id")?;
    let recovery_id: Uuid = row.try_get("recovery_id")?;
    let other_blocker: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND id <> $2
               AND execution_state IN ('failed', 'manual_reconcile')
         )",
    )
    .bind(recovery_id)
    .bind(action_id)
    .fetch_one(&mut **transaction)
    .await?;
    if other_blocker {
        return Ok(false);
    }
    let reset = sqlx::query(
        "UPDATE public.settlement_recovery_actions
         SET execution_state = 'enqueued', last_error_code = NULL,
             claim_token = NULL, claimed_at = NULL, claim_expires_at = NULL,
             claim_fencing = claim_fencing + 1, retry_count = 0,
             next_attempt_at = NULL, completed_at = NULL,
             state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE id = $1 AND delivery_id = $2 AND execution_state = $3
           AND prepare_command_id IS NOT NULL AND octet_length(prepare_request_digest) = 32
           AND activate_command_id IS NOT NULL AND octet_length(activate_request_digest) = 32
           AND proof_kind IS NULL AND proof_reference IS NULL
           AND proof_digest IS NULL AND proof_observed_at IS NULL
           AND (claim_token IS NULL OR claim_expires_at <= clock_timestamp())",
    )
    .bind(action_id)
    .bind(delivery_id)
    .bind(state)
    .execute(&mut **transaction)
    .await?;
    if reset.rows_affected() != 1 {
        return Ok(false);
    }
    let projected = sqlx::query(
        "UPDATE public.settlement_recoveries
         SET blocking_code = NULL, manual_reconcile_delivery_id = NULL,
             state_version = state_version + 1, updated_at = clock_timestamp()
         WHERE id = $1 AND stage <> 'completed' AND resolution IS NULL
           AND blocking_code = $2",
    )
    .bind(recovery_id)
    .bind(expected_error)
    .execute(&mut **transaction)
    .await?;
    Ok(projected.rows_affected() == 1)
}
