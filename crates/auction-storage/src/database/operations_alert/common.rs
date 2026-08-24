//! Operations alert 适配器共享的错误和时间转换辅助。
//!
//! 这里不持有业务状态，只把底层失败映射为安全应用错误并检查时间范围。

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn rollback_operations_alert(
    transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    message: &str,
) -> Result<(), ApplicationError> {
    let _ = transaction.rollback().await;
    Err(storage_error(message))
}

#[cfg(feature = "postgres")]
pub(super) fn chrono_duration(value: Duration) -> Result<chrono::Duration, ApplicationError> {
    chrono::Duration::from_std(value).map_err(|_| storage_error("operations alert 时间范围溢出"))
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "sqlite")]
pub(super) fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("operations alert 仅支持 PostgreSQL 完整档")
}
