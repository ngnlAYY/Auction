//! operations alert observation、批次 claim 和 delivery 状态端口模型。

use super::digest::{body_digest, canonical_summary};
use super::model::{OperationsAlertSourceObservation, OperationsAlertTarget};
use super::signals::OperationsAlertError;
use super::*;

/// 一次扫描完成后提交的完整 observation 集合。
#[derive(Debug, Clone)]
pub struct OperationsAlertObserveCommand {
    pub observations: Vec<OperationsAlertSourceObservation>,
    pub now: DateTime<Utc>,
    pub batch_window: Duration,
}

impl OperationsAlertObserveCommand {
    /// 校验 observation 顺序、时间和批次边界。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.observations.len() != 4
            || self.now.timestamp_nanos_opt().is_none()
            || self.batch_window < Duration::from_secs(1)
            || self.batch_window > Duration::from_secs(3_600)
        {
            return Err(OperationsAlertError::Invalid);
        }
        let expected = [
            OperationsAlertSource::Process,
            OperationsAlertSource::Delivery,
            OperationsAlertSource::Fulfillment,
            OperationsAlertSource::Recovery,
        ];
        // 四个来源共享同一 fingerprint 空间：跨来源重复同样代表同一 signal，
        // 必须在写入前拒绝，避免同一轮出现两条水位或两次通知。
        let mut fingerprints =
            std::collections::HashSet::with_capacity(expected.len() * MAX_OPERATIONS_ALERT_SIGNALS);
        for (observation, source) in self.observations.iter().zip(expected) {
            if observation.source != source {
                return Err(OperationsAlertError::Invalid);
            }
            observation.validate()?;
            if observation.signals.iter().any(|signal| !fingerprints.insert(signal.fingerprint)) {
                return Err(OperationsAlertError::Invalid);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct OperationsAlertClaimRequest {
    pub target: OperationsAlertTarget,
    pub now: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub max_items: usize,
    pub max_body_bytes: usize,
}

impl OperationsAlertClaimRequest {
    /// 校验 lease、目标和有界领取参数。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        self.target.validate()?;
        if self.now.timestamp_nanos_opt().is_none()
            || self.lease_until <= self.now
            || self.max_items == 0
            || self.max_items > MAX_OPERATIONS_ALERT_BATCH_ITEMS
            || self.max_body_bytes < 256
            || self.max_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// 告警批次中的冻结条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertBatchItem {
    pub fingerprint: [u8; 32],
    pub signal_version: i64,
    pub ordinal: i32,
    pub summary: String,
}

/// Worker 领取告警批次后持有的 lease/fencing 快照。
#[derive(Debug, Clone)]
pub struct OperationsAlertBatchClaim {
    pub batch_id: Uuid,
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub request_hash: RequestHash,
    pub body_digest: [u8; 32],
    pub summary: String,
    pub items: Vec<OperationsAlertBatchItem>,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    pub lease_until: DateTime<Utc>,
    pub attempt_count: u32,
}

impl OperationsAlertBatchClaim {
    /// 校验冻结正文、批次 identity 和 lease 边界。
    pub fn validate(&self, maximum_body_bytes: usize) -> Result<(), OperationsAlertError> {
        if self.batch_id.is_nil()
            || self.command_id.is_nil()
            || self.delivery_id.is_nil()
            || self.request_hash.as_bytes() == [0; 32]
            || self.body_digest == [0; 32]
            || self.claim_token.is_nil()
            || self.claim_fencing <= 0
            || self.attempt_count == 0
            || self.attempt_count > 3
            || self.lease_until.timestamp_nanos_opt().is_none()
            || self.items.is_empty()
            || self.items.len() > MAX_OPERATIONS_ALERT_BATCH_ITEMS
            || self.summary.is_empty()
            || self.summary.len() > maximum_body_bytes
            || body_digest(&self.summary) != self.body_digest
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.fingerprint == [0; 32]
                || item.signal_version <= 0
                || i32::try_from(index).ok().and_then(|index| index.checked_add(1))
                    != Some(item.ordinal)
                || !auction_domain::valid_text(&item.summary, 512)
            {
                return Err(OperationsAlertError::StoredResultCorrupt);
            }
        }
        if canonical_summary(&self.items, maximum_body_bytes)? != self.summary {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertEnqueuedBatch {
    pub batch_id: Uuid,
    pub delivery_id: Uuid,
    pub delivery_status: OperationsAlertDeliveryStatus,
}

/// 外部 delivery 的安全终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertDeliveryStatus {
    Succeeded,
    Failed,
    ManualReconcile,
}

impl OperationsAlertDeliveryStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ManualReconcile => "manual_reconcile",
        }
    }
}

/// delivery 创建阶段允许持久化的稳定失败类别。
/// 普通失败只用于决定是否退避重试，不会写入 pending 批次；只有依赖 identity 不一致或
/// 重试耗尽后，存储适配器才把稳定终态错误码写入批次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertDispatchErrorCode {
    Timeout,
    Canceled,
    Persistence,
    DependencyMismatch,
}

impl OperationsAlertDispatchErrorCode {
    /// 返回数据库状态机使用的稳定错误码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Canceled => "canceled",
            Self::Persistence => "persistence",
            Self::DependencyMismatch => "dependency_mismatch",
        }
    }
}
