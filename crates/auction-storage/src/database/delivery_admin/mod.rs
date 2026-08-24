//! PostgreSQL delivery Admin 适配器装配。
//!
//! 列表读取、人工命令事务和安全投影映射分别位于 `reads.rs`、`commands.rs` 和 `mapping.rs`；
//! 本文件只负责端口校验、后端能力分派和共享错误分类。SQLite Lite 明确不提供该能力。

use auction_application::{
    ApplicationError, DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminListRequest,
    DeliveryAdminPage, DeliveryAdminPort,
};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod commands;
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod reads;

impl DeliveryAdminPort for DatabaseStore {
    fn list(
        &self,
        request: DeliveryAdminListRequest,
    ) -> auction_application::ApplicationFuture<'_, DeliveryAdminPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(repository_error("delivery Admin 列表参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    reads::list(store.pool(), &request).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery Admin 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn execute(
        &self,
        command: DeliveryAdminCommand,
    ) -> auction_application::ApplicationFuture<'_, auction_application::DeliveryAdminResult> {
        Box::pin(async move {
            validate_command(&command)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    commands::execute(store.pool(), &command).await.map_err(ApplicationError::from)
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "delivery Admin 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

fn validate_command(command: &DeliveryAdminCommand) -> Result<(), ApplicationError> {
    if command.command_id.is_nil()
        || command.delivery_id.is_nil()
        || command.expected_fencing_version <= 0
        || command.actor_id.is_empty()
        || command.actor_id != command.actor_id.trim()
        || command
            .request_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value != value.trim())
        || command.request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error("delivery Admin 命令参数无效"));
    }
    match command.action {
        DeliveryAdminAction::Release
        | DeliveryAdminAction::RetryFailed
        | DeliveryAdminAction::ReconcileRetry
            if command.external_reference.is_none() && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileSucceeded
            if command
                .external_reference
                .as_ref()
                .is_some_and(|value| !value.is_empty() && value == value.trim())
                && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileFailed
            if command.external_reference.is_none()
                && command
                    .failure_code
                    .as_ref()
                    .is_some_and(|value| !value.is_empty() && value == value.trim()) => {}
        _ => return Err(repository_error("delivery Admin 命令结果字段不匹配")),
    }
    Ok(())
}

fn repository_error(message: &str) -> ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(feature = "postgres")]
pub(crate) fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "delivery Admin 记录不存在".to_owned(),
        _ => "delivery Admin 数据库操作失败".to_owned(),
    })
}
