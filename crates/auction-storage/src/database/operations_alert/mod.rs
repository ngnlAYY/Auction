//! PostgreSQL 完整档 operations alert 观察、批次和租约适配器；告警只读各 owner 已提交的安全水位，
//! 不参与竞价、结算或 delivery 的正确性决策。signal、批次和 delivery 同事务建立，Worker 之后经
//! `MailDeliveryEnqueuer` 幂等确认，数据库状态才是唯一权威。

use std::time::Duration;

use auction_application::{
    ApplicationError, ApplicationFuture, OperationsAlertBatchClaim, OperationsAlertClaimRequest,
    OperationsAlertDeliveryStatus, OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch,
    OperationsAlertError, OperationsAlertHealth, OperationsAlertObserveCommand,
    OperationsAlertRuntimeStatus, OperationsAlertSnapshot, OperationsAlertSourceReader,
    OperationsAlertStore,
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
                Self::Postgres(store) => snapshot_postgres(store, now).await,
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
        command: auction_application::OperationsAlertHostObserveCommand,
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
                Self::Postgres(store) => claim_postgres(store.pool(), &request).await,
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
                .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
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
                .validate(auction_application::MAX_OPERATIONS_ALERT_BODY_BYTES)
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
use snapshot::{observe_host_postgres, observe_postgres, snapshot_postgres};
