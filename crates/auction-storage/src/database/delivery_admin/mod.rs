//! Admin 履约查询与命令适配；复用应用状态机、持久命令账本和统一错误分类。

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminListRequest,
    DeliveryAdminPage, DeliveryAdminPort,
};

use super::DatabaseStore;
use uuid::Uuid;

mod commands;
mod detail;
mod mapping;
mod reads;
mod support;
mod transaction;

impl DeliveryAdminPort for DatabaseStore {
    fn list(
        &self,
        request: DeliveryAdminListRequest,
    ) -> auction_application::auction::ApplicationFuture<'_, DeliveryAdminPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            reads::list(self.pool(), &request).await.map_err(ApplicationError::from)
        })
    }

    fn detail(
        &self,
        delivery_id: Uuid,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        auction_application::delivery::DeliveryAdminDetail,
    > {
        Box::pin(async move {
            if delivery_id.is_nil() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            detail::detail(self.pool(), delivery_id).await.map_err(ApplicationError::from)
        })
    }

    fn player_support(
        &self,
        request: auction_application::delivery::PlayerSupportRequest,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        auction_application::delivery::PlayerSupportBundle,
    > {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            support::player_support(self.pool(), &request).await.map_err(ApplicationError::from)
        })
    }

    fn execute(
        &self,
        command: DeliveryAdminCommand,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        auction_application::delivery::DeliveryAdminResult,
    > {
        Box::pin(async move {
            validate_command(&command)?;
            commands::execute(self.pool(), &command).await.map_err(ApplicationError::from)
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

pub(crate) fn map_storage_error(_: sqlx::Error) -> DeliveryAdminError {
    DeliveryAdminError::Unavailable
}
