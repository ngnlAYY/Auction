//! Recovery relist/refund 共用的稳定命令、状态 fence 和审计写入辅助。

use crate::database::TrackedTransaction as Transaction;
use crate::database::operations::audit_health::AuditWriteResult;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::RecoveryRow;
use crate::database::recovery::commands::scope::RelistResult;
use auction_application::recovery::RecoveryCommandError;
use auction_domain::{CommandId, legacy_natural_key_uuid};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

pub(in crate::database::recovery::commands) fn scheduled_relist_command_id(
    recovery_id: Uuid,
) -> CommandId {
    let nil_uuid = Uuid::nil();
    CommandId::from_uuid(legacy_natural_key_uuid(
        "auction.recovery.relist-command.v1",
        &[recovery_id.as_bytes(), nil_uuid.as_bytes()],
    ))
}

pub(in crate::database::recovery::commands) async fn defer_recovery_relist(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    resolution: &str,
    retry_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let state_version = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET next_attempt_at = $1,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $3
             AND stage = 'relist_pending'
             AND state_version = $4
             AND resolution = $5
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
           RETURNING state_version"#,
    )
    .bind(retry_at)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(resolution)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    state_version.try_get("state_version").map_err(map_sql_error)
}

/// 永久不能重挂（旧赛季装备）时写入阻断码并退出自动重试；阶段保持 `relist_pending`。
pub(in crate::database::recovery::commands) async fn block_recovery_relist(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    resolution: &str,
    blocking_code: &str,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    let state_version = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET blocking_code = $1,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $3
             AND stage = 'relist_pending'
             AND state_version = $4
             AND resolution = $5
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
           RETURNING state_version"#,
    )
    .bind(blocking_code)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(resolution)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    state_version.try_get("state_version").map_err(map_sql_error)
}

pub(in crate::database::recovery::commands) async fn complete_recovery_relist(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    relist: &RelistResult,
    relisted_auction_id: Uuid,
    relisted_lot_id: Uuid,
    next_state_version: i64,
    now: DateTime<Utc>,
) -> Result<i64, RecoveryCommandError> {
    if !matches!(relist.stage, "refunding" | "completed") {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let completed_at = (relist.stage == "completed").then_some(now);
    let state_version = sqlx::query(
        r#"UPDATE public.settlement_recoveries
           SET stage = $1,
               relisted_auction_id = $2,
               relisted_lot_id = $3,
               next_attempt_at = NULL,
               completed_at = $4,
               state_version = $5,
               updated_at = $6
           WHERE id = $7
             AND stage = 'relist_pending'
             AND state_version = $8
             AND resolution = $9
             AND custody_proof_kind IS NOT NULL
             AND relisted_auction_id IS NULL
             AND relisted_lot_id IS NULL
             AND blocking_code IS NULL
             AND manual_reconcile_delivery_id IS NULL
           RETURNING state_version"#,
    )
    .bind(relist.stage)
    .bind(relisted_auction_id)
    .bind(relisted_lot_id)
    .bind(completed_at)
    .bind(next_state_version)
    .bind(now)
    .bind(recovery.id)
    .bind(recovery.state_version)
    .bind(relist.resolution)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;
    state_version.try_get("state_version").map_err(map_sql_error)
}

pub(in crate::database::recovery::commands) async fn insert_worker_recovery_audit(
    transaction: &mut Transaction<'_, Postgres>,
    recovery_id: Uuid,
    action: &str,
    occurred_at: DateTime<Utc>,
    metadata: Value,
) -> Result<(), RecoveryCommandError> {
    sqlx::query(
        r#"INSERT INTO public.audit_records
               (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                result, request_id, source, metadata)
           VALUES ($1, $2, 'system', 'recovery-worker', $3, 'settlement_recovery',
                   $4, 'success', $5, 'worker', $6)"#,
    )
    .bind(Uuid::new_v4())
    .bind(occurred_at)
    .bind(action)
    .bind(recovery_id.to_string())
    .bind(None::<String>)
    .bind(metadata)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_sql_error)?;
    Ok(())
}
