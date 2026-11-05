//! Admin 履约查询与命令适配；复用应用状态机、持久命令账本和统一错误分类。

use auction_application::{
    ApplicationError, DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError,
    DeliveryAdminListRequest, DeliveryAdminPage, DeliveryAdminPort,
};

use super::DatabaseStore;
use uuid::Uuid;

#[cfg(feature = "postgres")]
mod commands;
#[cfg(feature = "postgres")]
mod detail;
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod reads;
#[cfg(feature = "postgres")]
mod support;
#[cfg(feature = "postgres")]
mod transaction;

impl DeliveryAdminPort for DatabaseStore {
    fn list(
        &self,
        request: DeliveryAdminListRequest,
    ) -> auction_application::ApplicationFuture<'_, DeliveryAdminPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    reads::list(store.pool(), &request).await.map_err(ApplicationError::from)
                }
            }
        })
    }

    fn detail(
        &self,
        delivery_id: Uuid,
    ) -> auction_application::ApplicationFuture<'_, auction_application::DeliveryAdminDetail> {
        Box::pin(async move {
            if delivery_id.is_nil() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    detail::detail(store.pool(), delivery_id).await.map_err(ApplicationError::from)
                }
            }
        })
    }

    fn player_support(
        &self,
        request: auction_application::PlayerSupportRequest,
    ) -> auction_application::ApplicationFuture<'_, auction_application::PlayerSupportBundle> {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => support::player_support(store.pool(), &request)
                    .await
                    .map_err(ApplicationError::from),
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
            }
        })
    }
}

fn validate_command(command: &DeliveryAdminCommand) -> Result<(), DeliveryAdminError> {
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
        return Err(DeliveryAdminError::Invalid);
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
        _ => return Err(DeliveryAdminError::Invalid),
    }
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) fn map_storage_error(_: sqlx::Error) -> DeliveryAdminError {
    DeliveryAdminError::Unavailable
}
