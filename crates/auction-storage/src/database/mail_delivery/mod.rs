//! PostgreSQL MoogleMail delivery outbox 适配器的模块装配。
//!
//! 该 owner 只保存稳定操作、收件人和不含密钥的版本化 payload。普通入队、recovery 激活、
//! 命令校验和跨后端分派各自位于子模块；真正的远端 POST 仍由 Worker 的 DeliveryTransport
//! 负责，入队成功不等于外部发送成功。

use auction_application::{
    ApplicationError, MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryOrigin, MailDeliveryResult, RepositoryError,
};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod activation;
mod common;
#[cfg(feature = "postgres")]
mod disposition;
#[cfg(feature = "postgres")]
mod outbox;
mod validation;

#[cfg(feature = "postgres")]
use activation::activate_postgres;
#[cfg(feature = "postgres")]
pub(super) use disposition::has_return_intent;
#[cfg(feature = "postgres")]
use outbox::{enqueue_postgres, prepare_postgres};
use validation::validate_command;

impl MailDeliveryEnqueuer for DatabaseStore {
    fn enqueue(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    enqueue_postgres(store.pool(), &command).await.map_err(ApplicationError::from)
                }
            }
        })
    }

    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            if command.origin != MailDeliveryOrigin::SettlementRecoveryAction {
                return Err(RepositoryError::Failed(
                    "只有 recovery owner 可以准备两阶段 delivery".to_owned(),
                )
                .into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    prepare_postgres(store.pool(), &command).await.map_err(ApplicationError::from)
                }
            }
        })
    }

    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> auction_application::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            if activation.delivery_id.is_nil()
                || activation.prepare_command_id.is_nil()
                || activation.prepare_request_hash.as_bytes() == [0; 32]
                || activation.activate_command_id.is_nil()
                || activation.activate_request_hash.as_bytes() == [0; 32]
                || activation.activated_at.timestamp_nanos_opt().is_none()
            {
                return Err(RepositoryError::Failed(
                    "recovery delivery activation identity 无效".to_owned(),
                )
                .into());
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => activate_postgres(store.pool(), &activation)
                    .await
                    .map_err(ApplicationError::from),
            }
        })
    }
}

pub(crate) use outbox::{enqueue_in_transaction, payload};
