//! PostgreSQL 恢复前预检的迁移历史摘要、库名校验与错误映射；迁移历史与 schema 身份的读取
//! 由迁移 owner 提供。

use sqlx::Row;

use super::RecoveryPreflightError;
use crate::database::PostgresMigrationInventory;

pub(super) async fn read_migration_inventory(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
) -> Result<PostgresMigrationInventory, RecoveryPreflightError> {
    let rows = crate::database::migrations::read_postgres_history(&mut **transaction)
        .await
        .map_err(|source| query_error("migration_identity", source))?;
    let latest_version = rows
        .last()
        .map_or(Ok(0_i64), |row| row.try_get("version"))
        .map_err(|source| query_error("migration_summary", source))?;
    // preflight 同时用于备份与恢复后验；缺项、额外版本或未知 checksum 必须阻断成功回执。
    if !crate::database::migrations::postgres_history_matches(&rows, latest_version) {
        return Err(RecoveryPreflightError::MigrationHistoryInvalid);
    }
    // 完整前缀校验已逐行确认 success；摘要复用同一批历史，不另行查询。
    let migration_count =
        i64::try_from(rows.len()).map_err(|_| RecoveryPreflightError::MigrationHistoryInvalid)?;
    let latest_description = rows
        .last()
        .map(|row| row.try_get("description"))
        .transpose()
        .map_err(|source| query_error("migration_description", source))?;
    let expected_version = crate::database::postgres_expected_migration_version();
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
    if !crate::database::valid_confirmed_database_name(value) {
        return Err(RecoveryPreflightError::InvalidDatabaseName);
    }
    Ok(())
}
