//! Recovery 适配器的共享校验、摘要和 SQL 行映射；安全校验集中在此，避免各 action 各自解释字段。

#[cfg(feature = "postgres")]
use super::advance::DueRecoveryRow;
#[cfg(feature = "postgres")]
use super::*;
#[cfg(feature = "postgres")]
pub(super) use auction_domain::valid_text as valid_recovery_text;

pub(super) fn valid_failure_code(value: &str) -> bool {
    auction_domain::valid_text(value, 128)
}

#[cfg(feature = "postgres")]
pub(super) fn request_hash_from_optional(
    value: Option<Vec<u8>>,
) -> Result<Option<RequestHash>, auction_application::RepositoryError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: [u8; 32] =
        value.try_into().map_err(|_| repository_error("recovery action 请求摘要长度无效"))?;
    if bytes == [0; 32] {
        return Err(repository_error("recovery action 请求摘要不能为零"));
    }
    Ok(Some(RequestHash::new(bytes)))
}

#[cfg(feature = "postgres")]
pub(super) fn valid_external_reference(value: &str) -> bool {
    auction_domain::valid_text(value, 512)
}

#[cfg(feature = "postgres")]
pub(super) fn map_due_recovery_row(
    row: sqlx::postgres::PgRow,
) -> Result<DueRecoveryRow, auction_application::RepositoryError> {
    Ok(DueRecoveryRow {
        id: row.try_get("id").map_err(map_storage_error)?,
        stage: row.try_get("stage").map_err(map_storage_error)?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        reminder_count: row.try_get("reminder_count").map_err(map_storage_error)?,
        reminder_limit: row.try_get("reminder_limit").map_err(map_storage_error)?,
        world_id: row.try_get("world_id").map_err(map_storage_error)?,
        payment_execution_state: row
            .try_get("payment_execution_state")
            .map_err(map_storage_error)?,
        payment_completed_at: row.try_get("payment_completed_at").map_err(map_storage_error)?,
        equipment_execution_state: row
            .try_get("equipment_execution_state")
            .map_err(map_storage_error)?,
        equipment_completed_at: row.try_get("equipment_completed_at").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        payment_sent_proven: row.try_get("payment_sent_proven").map_err(map_storage_error)?,
    })
}

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    let context = match &error {
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001") => {
            "PostgreSQL recovery Worker 发生序列化冲突"
        }
        _ => "PostgreSQL recovery case 推进失败",
    };
    auction_application::RepositoryError::Failed(crate::database::sql_error_context(
        &error, context,
    ))
}
