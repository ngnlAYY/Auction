//! Admin runtime 命令账本的 PostgreSQL 错误分类。

use auction_application::admin::AdminRuntimeCommandError;
use auction_application::core::ApplicationError;

pub(super) fn command_failed(message: &str) -> ApplicationError {
    AdminRuntimeCommandError::Failed(message.to_owned()).into()
}

/// SQL 失败仍归为 `Failed`，消息附带 [`crate::database::sql_error_context`] 的安全投影
/// （sqlx 类别、SQLSTATE、约束与表名），连接失败、锁超时与列解码失败在诊断中可区分；
/// 驱动原文不进入消息（不变量 7）。
pub(super) fn command_sql_failed(error: &sqlx::Error, context: &'static str) -> ApplicationError {
    command_failed(&crate::database::sql_error_context(error, context))
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
        _ => command_sql_failed(&error, "PostgreSQL runtime 配置命令操作失败"),
    }
}

#[cfg(test)]
mod tests {
    use auction_application::admin::AdminRuntimeCommandError;
    use auction_application::core::ApplicationError;

    use super::command_sql_failed;

    fn failed_message(error: ApplicationError) -> String {
        match error {
            ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Failed(message)) => {
                message
            }
            other => panic!("SQL 失败必须保持 Failed 分类：{other:?}"),
        }
    }

    #[test]
    fn sql_failures_keep_stage_and_safe_projection() {
        let timeout = failed_message(command_sql_failed(
            &sqlx::Error::PoolTimedOut,
            "开启 runtime 配置命令事务失败",
        ));
        assert!(timeout.starts_with("开启 runtime 配置命令事务失败 ["), "{timeout}");
        assert!(timeout.contains("sqlx_kind=PoolTimedOut"), "{timeout}");
        assert!(timeout.contains("sqlstate=none"), "{timeout}");

        let decode = failed_message(command_sql_failed(
            &sqlx::Error::ColumnNotFound("secret_column".to_owned()),
            "runtime 命令标识读取失败",
        ));
        assert!(decode.contains("sqlx_kind=ColumnNotFound"), "{decode}");
        assert!(!decode.contains("secret_column"), "驱动负载不得进入消息：{decode}");
    }
}
