//! Operations alert 适配器共享的错误和时间转换；不持有业务状态，只做安全错误映射与范围校验。

use super::*;
pub(super) use crate::database::storage_error;

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
