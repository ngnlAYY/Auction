//! PostgreSQL 恢复前预检的事务策略与数据库证据查询。

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{PostgresMigrationInventory, PostgresSchemaIdentity, RecoveryPreflightError};

/// 为调用方以 `begin_read_snapshot` 开启的只读快照设置语句与锁等待上限。
pub(super) async fn set_read_only_limits(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), RecoveryPreflightError> {
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("statement_timeout", source))?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("lock_timeout", source))?;
    Ok(())
}

pub(super) async fn read_schema_identity(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresSchemaIdentity, RecoveryPreflightError> {
    let row = sqlx::query(
        "SELECT application, baseline_version, installed_at
         FROM public.auction_schema_identity
         WHERE singleton
         LIMIT 2",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| query_error("schema_identity", source))?
    .ok_or(RecoveryPreflightError::SchemaIdentityInvalid)?;
    let application: String =
        row.try_get("application").map_err(|source| query_error("schema_identity", source))?;
    let baseline_version: i64 =
        row.try_get("baseline_version").map_err(|source| query_error("schema_identity", source))?;
    let installed_at: DateTime<Utc> =
        row.try_get("installed_at").map_err(|source| query_error("schema_identity", source))?;
    if application != "auction" || baseline_version <= 0 {
        return Err(RecoveryPreflightError::SchemaIdentityInvalid);
    }
    Ok(PostgresSchemaIdentity { application, baseline_version, installed_at })
}

pub(super) async fn read_migration_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresMigrationInventory, RecoveryPreflightError> {
    let rows = sqlx::query(
        "SELECT version, description, success, checksum FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("migration_identity", source))?;
    let latest_version = rows
        .last()
        .map_or(Ok(0_i64), |row| row.try_get("version"))
        .map_err(|source| query_error("migration_summary", source))?;
    // preflight 同时用于备份与恢复后验；缺项、额外版本或未知 checksum 必须阻断成功回执。
    if !super::super::migrations::postgres_history_matches(&rows, latest_version) {
        return Err(RecoveryPreflightError::MigrationHistoryInvalid);
    }
    // 完整前缀校验已逐行确认 success；摘要直接取同一批历史，不再发起两次重复查询。
    let migration_count =
        i64::try_from(rows.len()).map_err(|_| RecoveryPreflightError::MigrationHistoryInvalid)?;
    let latest_description = rows
        .last()
        .map(|row| row.try_get("description"))
        .transpose()
        .map_err(|source| query_error("migration_description", source))?;
    let expected_version = super::super::postgres_expected_migration_version();
    Ok(PostgresMigrationInventory {
        migration_count,
        latest_version,
        latest_description,
        all_success: true,
        expected_version,
        current: latest_version == expected_version,
    })
}

pub(super) fn query_error(stage: &'static str, source: sqlx::Error) -> RecoveryPreflightError {
    RecoveryPreflightError::Query { stage, source }
}

pub(super) fn validate_database_name(value: &str) -> Result<(), RecoveryPreflightError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 63
        || value.chars().any(char::is_control)
    {
        return Err(RecoveryPreflightError::InvalidDatabaseName);
    }
    Ok(())
}
