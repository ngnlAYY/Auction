//! Admin runtime 命令账本的错误分类和 SQLite Lite 能力边界。

use auction_application::ApplicationError;

#[cfg(feature = "postgres")]
use auction_application::AdminRuntimeCommandError;

#[cfg(feature = "postgres")]
pub(super) fn command_failed(message: &str) -> ApplicationError {
    AdminRuntimeCommandError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
pub(super) fn command_corrupt(message: String) -> ApplicationError {
    AdminRuntimeCommandError::Failed(format!("runtime 命令账本损坏：{message}")).into()
}

#[cfg(feature = "postgres")]
pub(super) fn map_command_sql_error(error: sqlx::Error) -> ApplicationError {
    match error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminRuntimeCommandError::Conflict.into()
        }
        _ => command_failed("PostgreSQL runtime 配置命令操作失败"),
    }
}

#[cfg(feature = "sqlite")]
pub(super) fn lite_unavailable() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("Admin runtime 配置更新仅支持 PostgreSQL 完整档")
}
