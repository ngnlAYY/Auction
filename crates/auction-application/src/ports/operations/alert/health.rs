//! operations alert Dashboard 健康投影与存储窄端口。

use super::delivery::{
    OperationsAlertBatchClaim, OperationsAlertClaimRequest, OperationsAlertDeliveryStatus,
    OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch, OperationsAlertObserveCommand,
};
use super::digest::valid_health_error_code;
use super::model::OperationsAlertSnapshot;
use super::signals::OperationsAlertError;
use super::*;

/// Dashboard 使用的 operations alert 健康投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationsAlertHealth {
    pub enabled: bool,
    pub active_signal_count: i64,
    pub due_signal_count: i64,
    pub claimed_batch_count: i64,
    pub succeeded_batch_count: i64,
    pub failed_batch_count: i64,
    pub manual_reconcile_count: i64,
    pub last_observed_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_error_code: String,
}

impl OperationsAlertHealth {
    /// 构造告警关闭时的中性投影。
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            active_signal_count: 0,
            due_signal_count: 0,
            claimed_batch_count: 0,
            succeeded_batch_count: 0,
            failed_batch_count: 0,
            manual_reconcile_count: 0,
            last_observed_at: None,
            last_success_at: None,
            last_failure_at: None,
            last_error_code: String::new(),
        }
    }

    /// 校验 Dashboard 计数和时间关系。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if [
            self.active_signal_count,
            self.due_signal_count,
            self.claimed_batch_count,
            self.succeeded_batch_count,
            self.failed_batch_count,
            self.manual_reconcile_count,
        ]
        .iter()
        .any(|value| *value < 0)
            || self.due_signal_count > self.active_signal_count
            || !valid_health_error_code(&self.last_error_code)
            || (!self.last_error_code.is_empty() && self.last_failure_at.is_none())
            || [
                self.last_observed_at.as_ref(),
                self.last_success_at.as_ref(),
                self.last_failure_at.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|value| value.timestamp_nanos_opt().is_none())
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        if !self.enabled
            && (self.active_signal_count != 0
                || self.due_signal_count != 0
                || self.claimed_batch_count != 0
                || self.succeeded_batch_count != 0
                || self.failed_batch_count != 0
                || self.manual_reconcile_count != 0
                || self.last_observed_at.is_some()
                || self.last_success_at.is_some()
                || self.last_failure_at.is_some()
                || !self.last_error_code.is_empty())
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 从数据库/observability owner 读取告警来源的窄端口。
pub trait OperationsAlertSourceReader: Send + Sync {
    fn snapshot(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, OperationsAlertSnapshot>;
}

/// 保存 signal、批次、lease 和终态的窄端口。
pub trait OperationsAlertStore: Send + Sync {
    fn observe(&self, command: OperationsAlertObserveCommand) -> ApplicationFuture<'_, ()>;

    fn claim_due_batch(
        &self,
        request: OperationsAlertClaimRequest,
    ) -> ApplicationFuture<'_, Option<OperationsAlertBatchClaim>>;

    fn mark_enqueued(
        &self,
        claim: OperationsAlertBatchClaim,
        cooldown: Duration,
    ) -> ApplicationFuture<'_, ()>;

    fn mark_claim_failed(
        &self,
        claim: OperationsAlertBatchClaim,
        error_code: OperationsAlertDispatchErrorCode,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, ()>;

    fn list_enqueued(&self, limit: u16)
    -> ApplicationFuture<'_, Vec<OperationsAlertEnqueuedBatch>>;

    fn mark_terminal(
        &self,
        batch: OperationsAlertEnqueuedBatch,
        status: OperationsAlertDeliveryStatus,
        completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()>;

    fn health(&self) -> ApplicationFuture<'_, OperationsAlertHealth>;
}
