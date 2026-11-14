//! Admin runtime 命令账本的 PostgreSQL 错误分类。

use auction_application::admin::AdminRuntimeCommandError;
use auction_application::core::ApplicationError;

pub(super) fn command_failed(message: &str) -> ApplicationError {
    AdminRuntimeCommandError::Failed(message.to_owned()).into()
}

pub(super) fn command_corrupt(message: String) -> ApplicationError {
    AdminRuntimeCommandError::Failed(format!("runtime 命令账本损坏：{message}")).into()
}

pub(super) fn map_command_sql_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            AdminRuntimeCommandError::Conflict.into()
        }
        _ => command_failed(&crate::database::sql_error_context(
            &error,
            "PostgreSQL runtime 配置命令操作失败",
        )),
    }
}
