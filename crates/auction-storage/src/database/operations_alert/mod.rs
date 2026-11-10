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

const MAX_ATTEMPTS: u32 = 3;

impl OperationsAlertSourceReader for DatabaseStore {
    fn snapshot(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, OperationsAlertSnapshot> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(storage_error("operations alert 观察时间无效"));
            }
            // 按运行时开关选择处置读侧口径（legacy/shadow/classified）；
            // 分类口径不再额外查询，影子口径只记录差异。
            let snapshot = snapshot_postgres(self, now).await?;
            apply_read_mode(self, now, snapshot).await
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
            publish_runtime_status_postgres(self.pool(), status).await
        })
    }

    fn observe(&self, command: OperationsAlertObserveCommand) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            observe_postgres(self.pool(), &command).await
        })
    }

    fn observe_host(
        &self,
        command: auction_application::operations::OperationsAlertHostObserveCommand,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            command.validate().map_err(ApplicationError::from)?;
            observe_host_postgres(self.pool(), &command).await
        })
    }

    fn claim_due_batch(
        &self,
        request: OperationsAlertClaimRequest,
    ) -> ApplicationFuture<'_, Option<OperationsAlertBatchClaim>> {
        Box::pin(async move {
            request.validate().map_err(ApplicationError::from)?;
            claim_postgres(self.pool(), self.notification_metrics(), &request).await
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
            mark_enqueued_postgres(self.pool(), &claim, cooldown).await
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
            mark_claim_failed_postgres(self.pool(), &claim, error_code, retry_delay).await
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
            list_enqueued_postgres(self.pool(), limit).await
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
            mark_terminal_postgres(self.pool(), &batch, status, completed_at).await
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
            claim_due_webhook_postgres(self.pool(), &request).await
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
            finish_webhook_postgres(self.pool(), &dispatch, finish).await
        })
    }

    fn health(&self) -> ApplicationFuture<'_, OperationsAlertHealth> {
        Box::pin(async move { health_postgres(self.pool()).await })
    }
}

use super::TrackedPool;

mod claims;
mod common;
mod console;
mod delivery;
mod health;
mod snapshot;
mod webhook;

use claims::claim_postgres;
use common::*;
use delivery::{
    list_enqueued_postgres, mark_claim_failed_postgres, mark_enqueued_postgres,
    mark_terminal_postgres,
};
use health::{health_postgres, publish_runtime_status_postgres};
use snapshot::{apply_read_mode, observe_host_postgres, observe_postgres, snapshot_postgres};
use webhook::{claim_due_webhook_postgres, finish_webhook_postgres};
