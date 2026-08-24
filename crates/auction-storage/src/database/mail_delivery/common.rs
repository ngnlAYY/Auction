//! MoogleMail delivery PostgreSQL 错误分类辅助。

#[cfg(feature = "postgres")]
use auction_application::RepositoryError;

#[cfg(feature = "postgres")]
pub(super) fn map_storage_error(error: sqlx::Error) -> RepositoryError {
    RepositoryError::Failed(match error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "MoogleMail delivery 事务发生序列化冲突".to_owned()
        }
        _ => "MoogleMail delivery outbox 操作失败".to_owned(),
    })
}

#[cfg(feature = "postgres")]
pub(super) fn repository_error(message: &str) -> RepositoryError {
    RepositoryError::Failed(message.to_owned())
}
