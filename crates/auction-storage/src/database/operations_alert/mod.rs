//! PostgreSQL 完整档 operations alert 观察、批次和租约适配器；告警只读各 owner 已提交的安全水位，
//! 不参与竞价、结算或 delivery 的正确性决策。signal、批次和 delivery 同事务建立，Worker 之后经
//! `MailDeliveryEnqueuer` 幂等确认，数据库状态才是唯一权威。

use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertBatchClaim, OperationsAlertClaimRequest, OperationsAlertDeliveryStatus,
    OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch, OperationsAlertError,
    OperationsAlertHealth, OperationsAlertObserveCommand, OperationsAlertRuntimeStatus,
    OperationsAlertSnapshot, OperationsAlertSourceReader, OperationsAlertStore,
    OperationsAlertWebhookClaimRequest, OperationsAlertWebhookDispatch,
    OperationsAlertWebhookFinish,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
const MAX_ATTEMPTS: u32 = 3;

impl OperationsAlertSourceReader for DatabaseStore {
    fn snapshot(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, OperationsAlertSnapshot> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(storage_error("operations alert 观察时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    // W2.5：按运行时开关选择处置读侧口径（legacy/shadow/classified）；
                    // 分类口径不再额外查询，影子口径只记录差异。
                    let snapshot = snapshot_postgres(store, now).await?;
                    apply_read_mode(store, now, snapshot).await
                }
            }
        })
    }
}

impl OperationsAlertStore for DatabaseStore {
    fn publish_runtime_status(
        &self,
        status: OperationsAlertRuntimeStatus,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            status.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    publish_runtime_status_postgres(store.pool(), status).await
                }
            }
        })
    }

    fn observe(&self, command: OperationsAlertObserveCommand) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => observe_postgres(store.pool(), &command).await,
            }
        })
    }

    fn observe_host(
        &self,
        command: auction_application::operations::OperationsAlertHostObserveCommand,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => observe_host_postgres(store.pool(), &command).await,
            }
        })
    }

    fn claim_due_batch(
        &self,
        request: OperationsAlertClaimRequest,
    ) -> ApplicationFuture<'_, Option<OperationsAlertBatchClaim>> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_postgres(store.pool(), store.notification_metrics(), &request).await
                }
            }
        })
    }

    fn mark_enqueued(
        &self,
        claim: OperationsAlertBatchClaim,
        cooldown: Duration,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            claim
                .validate(auction_application::operations::MAX_OPERATIONS_ALERT_BODY_BYTES)
                .map_err(ApplicationError::from)?;
            if cooldown < Duration::from_secs(1) || cooldown > Duration::from_secs(7 * 24 * 3_600) {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_enqueued_postgres(store.pool(), &claim, cooldown).await
                }
            }
        })
    }

    fn mark_claim_failed(
        &self,
        claim: OperationsAlertBatchClaim,
        error_code: OperationsAlertDispatchErrorCode,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            claim
                .validate(auction_application::operations::MAX_OPERATIONS_ALERT_BODY_BYTES)
                .map_err(ApplicationError::from)?;
            if retry_delay < Duration::from_secs(1) || retry_delay > Duration::from_secs(24 * 3_600)
            {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_claim_failed_postgres(store.pool(), &claim, error_code, retry_delay).await
                }
            }
        })
    }

    fn list_enqueued(
        &self,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<OperationsAlertEnqueuedBatch>> {
        Box::pin(async move {
            if limit == 0 || limit > 64 {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_enqueued_postgres(store.pool(), limit).await,
            }
        })
    }

    fn mark_terminal(
        &self,
        batch: OperationsAlertEnqueuedBatch,
        status: OperationsAlertDeliveryStatus,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if batch.batch_id.is_nil()
                || batch.delivery_id.is_nil()
                || completed_at.timestamp_nanos_opt().is_none()
            {
                return Err(ApplicationError::OperationsAlert(OperationsAlertError::Invalid));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    mark_terminal_postgres(store.pool(), &batch, status, completed_at).await
                }
            }
        })
    }

    fn claim_due_webhook(
        &self,
        request: OperationsAlertWebhookClaimRequest,
    ) -> ApplicationFuture<'_, Option<OperationsAlertWebhookDispatch>> {
        Box::pin(async move {
            if request.now.timestamp_nanos_opt().is_none()
                || request.lease_until.timestamp_nanos_opt().is_none()
                || request.lease_until <= request.now
            {
                return Err(storage_error("webhook 领取时间无效"));
            }
            auction_application::operations::validate_webhook_template(&request.template)
                .map_err(ApplicationError::from)?;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => claim_due_webhook_postgres(store.pool(), &request).await,
            }
        })
    }

    fn finish_webhook(
        &self,
        dispatch: OperationsAlertWebhookDispatch,
        finish: OperationsAlertWebhookFinish,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if dispatch.delivery_id.is_nil()
                || dispatch.command_id.is_nil()
                || dispatch.claim_token.is_nil()
                || dispatch.claim_fencing <= 0
            {
                return Err(storage_error("webhook 终态身份无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    finish_webhook_postgres(store.pool(), &dispatch, finish).await
                }
            }
        })
    }

    fn health(&self) -> ApplicationFuture<'_, OperationsAlertHealth> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => health_postgres(store.pool()).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
use super::PostgresStore;
#[cfg(feature = "postgres")]
use super::TrackedPool;

#[cfg(feature = "postgres")]
mod claims;
mod common;
#[cfg(feature = "postgres")]
mod console;
#[cfg(feature = "postgres")]
mod delivery;
#[cfg(feature = "postgres")]
mod health;
#[cfg(feature = "postgres")]
mod snapshot;
mod webhook;

#[cfg(feature = "postgres")]
use claims::claim_postgres;
use common::*;
#[cfg(feature = "postgres")]
use delivery::{
    list_enqueued_postgres, mark_claim_failed_postgres, mark_enqueued_postgres,
    mark_terminal_postgres,
};
#[cfg(feature = "postgres")]
use health::{health_postgres, publish_runtime_status_postgres};
#[cfg(feature = "postgres")]
use snapshot::{apply_read_mode, observe_host_postgres, observe_postgres, snapshot_postgres};
use webhook::{claim_due_webhook_postgres, finish_webhook_postgres};
