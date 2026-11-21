//! 数据库清理的目标预检、分批事务、审计追加与安全失败投影。

use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::model::{RetentionPurgeError, RetentionPurgeOutcome, RetentionPurgeReport};
use super::{AUDIT_STATEMENT_TIMEOUT, LOCK_TIMEOUT, STATEMENT_TIMEOUT};
use crate::database::DatabaseStore;
use crate::database::TrackedPool;
use crate::database::operations::audit_health::AuditWriteResult as _;

/// 执行次序：先清理可能解除事件外键阻断的失败记录，再清理事件，其他族保持政策顺序。
pub(super) fn execution_rank(dataset_family: &str) -> u8 {
    match dataset_family {
        "realtime_failures" => 0,
        "realtime_events" => 2,
        _ => 1,
    }
}

pub(super) fn purge_failure_code(error: &RetentionPurgeError) -> &'static str {
    match error {
        RetentionPurgeError::Query { stage, .. } => match *stage {
            "begin_batch" => "batch_begin_failed",
            "delete_batch" => "delete_batch_failed",
            "count_remaining" => "remaining_count_failed",
            _ => "query_failed",
        },
        RetentionPurgeError::Commit { .. } => "batch_commit_failed",
        RetentionPurgeError::CountOverflow => "count_overflow",
        _ => "purge_failed",
    }
}

pub(super) async fn verify_target(
    store: &DatabaseStore,
    database_name: &str,
) -> Result<(), RetentionPurgeError> {
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(store.pool())
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "database_identity", source })?;
    if actual != database_name {
        return Err(RetentionPurgeError::DatabaseMismatch {
            expected: database_name.to_owned(),
            actual,
        });
    }
    require_current_migrations(store.pool()).await
}

async fn require_current_migrations(
    pool: &TrackedPool<sqlx::Postgres>,
) -> Result<(), RetentionPurgeError> {
    let expected_version = crate::database::postgres_expected_migration_version();
    let marker_exists = crate::database::migrations::evidence::postgres_history_present(pool)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "migration_marker", source })?;
    if !marker_exists {
        return Err(RetentionPurgeError::MigrationNotCurrent);
    }
    let migrations = crate::database::migrations::evidence::read_postgres_history(pool)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "migration_identity", source })?;
    if !crate::database::migrations::history::postgres_history_matches(
        &migrations,
        expected_version,
    ) {
        return Err(RetentionPurgeError::MigrationNotCurrent);
    }
    Ok(())
}

pub(super) async fn delete_batch(
    store: &DatabaseStore,
    statement: &'static str,
    cutoff: Timestamp,
    limit: i64,
) -> Result<(i64, i64), RetentionPurgeError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "begin_batch", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "statement_timeout", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}'")))
        .execute(&mut *transaction)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "lock_timeout", source })?;
    let row = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(cutoff)
        .bind(limit)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    let rows: i64 = row
        .try_get("deleted_rows")
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    let bytes: i64 = row
        .try_get("deleted_bytes")
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    transaction
        .commit()
        .await
        .map_err(|source| RetentionPurgeError::Commit { stage: "commit_batch", source })?;
    Ok((rows, bytes))
}

/// 追加清理审计。维护类审计保持 `actor_type='system'`、`source='system'`；CLI 发起时只把主机
/// 操作者身份并入 metadata，使唯一删除业务库行的命令可追溯到主机账号。
pub(super) async fn append_audit(
    pool: &TrackedPool<sqlx::Postgres>,
    report: &RetentionPurgeReport,
    operator_identity: Option<&auction_application::core::CliOperatorIdentity>,
) -> Result<Uuid, RetentionPurgeError> {
    let id = Uuid::new_v4();
    let families = report
        .families
        .iter()
        .map(|family| {
            serde_json::json!({
                "dataset_family": family.dataset_family,
                "retention_kind": family.retention_kind,
                "approved_rows": family.approved_rows,
                "approved_bytes": family.approved_bytes,
                "deleted_rows": family.deleted_rows,
                "deleted_bytes": family.deleted_bytes,
                "outcome": family.outcome,
                "tables": family.tables,
            })
        })
        .collect::<Vec<_>>();
    let metadata = auction_application::core::cli_audit_metadata(
        operator_identity,
        serde_json::json!({
            "database_name": report.database_name,
            "dry_run_run_id": report.dry_run_run_id,
            "purge_run_id": report.run_id,
            "policy_version": report.policy_version,
            "policy_sha256": report.policy_sha256,
            "batch_size": report.batch_size,
            "outcome": report.outcome,
            "failure_code": report.failure_code,
            "families": families,
            "deleted_rows": report.deleted_rows,
            "deleted_row_bytes": report.deleted_row_bytes,
            "filesystem": report.filesystem,
            "mutation_executed": report.mutation_executed,
        }),
    );
    let mut transaction = pool
        .begin()
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "audit_begin", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{AUDIT_STATEMENT_TIMEOUT}'"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "audit_timeout", source })?;
    sqlx::query(
        "INSERT INTO public.audit_records
             (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', 'auction-cli', 'auction.retention.purge',
                 'retention_purge', $2, $3, 'system', $4)",
    )
    .bind(id)
    .bind(report.run_id.to_string())
    .bind(if report.outcome == RetentionPurgeOutcome::Complete { "success" } else { "failure" })
    .bind(metadata)
    .execute(&mut *transaction)
    .await
    .record_audit_failure()
    .map_err(|source| RetentionPurgeError::Query { stage: "audit_insert", source })?;
    transaction
        .commit()
        .await
        .map_err(|source| RetentionPurgeError::Commit { stage: "audit_commit", source })?;
    Ok(id)
}

/// 回执只保留失败码；数据库错误的阶段、SQLSTATE、约束与表名等安全投影进日志，驱动原文不进日志。
pub(super) fn log_purge_failure(error: &RetentionPurgeError, context: &'static str) {
    match error {
        RetentionPurgeError::Query { stage, source }
        | RetentionPurgeError::Commit { stage, source } => {
            let context = crate::database::sql_error_context(source, context);
            tracing::warn!(kind = "retention_purge_failed", stage, context, "保留清理步骤失败");
        }
        other => {
            tracing::warn!(kind = "retention_purge_failed", error = %other, context, "保留清理步骤失败");
        }
    }
}

pub(in crate::database::maintenance::retention::purge) fn add(
    left: i64,
    right: i64,
) -> Result<i64, RetentionPurgeError> {
    left.checked_add(right).ok_or(RetentionPurgeError::CountOverflow)
}
