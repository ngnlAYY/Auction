//! PostgreSQL MoogleMail delivery outbox 适配器的模块装配；本 owner 只保存稳定操作、收件人和
//! 不含密钥的版本化 payload。远端 POST 仍由 Worker transport 执行，入队成功不等于外部发送成功。

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::mail::{
    MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOrigin,
    MailDeliveryResult,
};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod activation;
mod common;
#[cfg(feature = "postgres")]
mod disposition;
#[cfg(feature = "postgres")]
mod outbox;
#[cfg(feature = "postgres")]
mod preparation;
mod validation;

#[cfg(feature = "postgres")]
use activation::activate_postgres;
#[cfg(feature = "postgres")]
pub(crate) use common::MailDeliveryTransactionError;
#[cfg(feature = "postgres")]
pub(super) use disposition::has_return_intent;
use outbox::enqueue_within_limit_postgres;
#[cfg(feature = "postgres")]
use outbox::{enqueue_postgres, prepare_postgres};
use validation::validate_command;

impl MailDeliveryEnqueuer for DatabaseStore {
    fn enqueue(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::auction::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            {
                enqueue_postgres(self.pool(), &command, self.notification_metrics())
                    .await
                    .map_err(ApplicationError::from)
            }
        })
    }

    fn enqueue_within_limit(
        &self,
        command: MailDeliveryCommand,
        limit: auction_application::mail::MailNoticeLimit,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        auction_application::mail::MailNoticeEnqueue,
    > {
        Box::pin(async move {
            validate_command(&command)?;
            if limit.maximum > 0
                && !limit.counts(
                    command
                        .template
                        .ok_or(auction_application::mail::MailProcessingError::InvalidDecision)?,
                )
            {
                return Err(auction_application::core::RepositoryError::Failed(
                    "回信预算模板集合不包含当前通知".to_owned(),
                )
                .into());
            }
            {
                enqueue_within_limit_postgres(
                    self.pool(),
                    &command,
                    limit,
                    self.notification_metrics(),
                )
                .await
                .map_err(ApplicationError::from)
            }
        })
    }

    fn find_recovery_preparation(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::auction::ApplicationFuture<'_, Option<MailDeliveryResult>> {
        Box::pin(async move {
            validate_command(&command)?;
            if command.origin != MailDeliveryOrigin::SettlementRecoveryAction
                || command.operation != auction_application::mail::MailDeliveryOperation::Take
            {
                return Err(
                    RepositoryError::Failed("旧准备查询只接受 recovery take".to_owned()).into()
                );
            }
            {
                preparation::find_postgres(self.pool(), &command)
                    .await
                    .map_err(ApplicationError::from)
            }
        })
    }

    fn prepare_recovery(
        &self,
        command: MailDeliveryCommand,
    ) -> auction_application::auction::ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            validate_command(&command)?;
            if command.origin != MailDeliveryOrigin::SettlementRecoveryAction {
                return Err(RepositoryError::Failed(
                    "只有 recovery owner 可以准备两阶段 delivery".to_owned(),
                )
                .into());
            }
            {
                prepare_postgres(self.pool(), &command, self.notification_metrics())
                    .await
                    .map_err(ApplicationError::from)
            }
        })
    }

    fn activate_recovery(
        &self,
        activation: MailDeliveryActivation,
    ) -> auction_application::auction::ApplicationFuture<'_, MailDeliveryResult> {
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
            activate_postgres(self.pool(), &activation).await.map_err(ApplicationError::from)
        })
    }
}

pub(crate) use outbox::{enqueue_in_transaction, payload};
