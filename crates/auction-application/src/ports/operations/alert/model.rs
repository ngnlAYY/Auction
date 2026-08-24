//! operations alert 的来源观察、健康快照和策略模型。

use super::digest::valid_age_threshold;
use super::signals::{OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource};
use super::*;

/// 一次来源的完整观察集合；不可用时只保留固定 `source_unavailable` signal。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertSourceObservation {
    pub source: OperationsAlertSource,
    pub available: bool,
    pub signals: Vec<OperationsAlertSignal>,
}

impl OperationsAlertSourceObservation {
    /// 构造可用来源观察。
    pub fn available(
        source: OperationsAlertSource,
        signals: Vec<OperationsAlertSignal>,
    ) -> Result<Self, OperationsAlertError> {
        let result = Self { source, available: true, signals };
        result.validate()?;
        Ok(result)
    }

    /// 构造不暴露底层错误的不可用来源观察。
    pub fn unavailable(source: OperationsAlertSource) -> Result<Self, OperationsAlertError> {
        let signal = OperationsAlertSignal::new(
            OperationsAlertKind::SourceUnavailable,
            source.as_str(),
            source,
            1,
            0,
        )?;
        Ok(Self { source, available: false, signals: vec![signal] })
    }

    /// 校验来源完整性与 signal 归属。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.signals.len() > MAX_OPERATIONS_ALERT_SIGNALS {
            return Err(OperationsAlertError::Invalid);
        }
        if !self.available
            && (self.signals.len() != 1
                || self.signals[0].kind != OperationsAlertKind::SourceUnavailable)
        {
            return Err(OperationsAlertError::Invalid);
        }
        if self
            .signals
            .iter()
            .any(|signal| signal.validate().is_err() || signal.source != self.source)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// Worker 观察到的进程安全状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertProcessState {
    pub role: String,
    pub status: String,
}

/// Process source 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertProcessSnapshot {
    pub processes: Vec<OperationsAlertProcessState>,
    pub audit_failure_count: i64,
}

/// Delivery source 快照；operations alert 自己的 delivery 不应计入此水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertDeliverySnapshot {
    pub pending: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    pub last_update_at: Option<DateTime<Utc>>,
}

/// Settlement fulfillment source 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertFulfillmentSnapshot {
    pub pending: i64,
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_backlog_age_seconds: i64,
}

/// Recovery 每个 world 的余额和 evidence 安全状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    pub evidence_age_seconds: i64,
    pub balance_observed_at: Option<DateTime<Utc>>,
    pub evidence_last_success_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Recovery source 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoverySnapshot {
    pub payment_overdue_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
    pub worlds: Vec<OperationsAlertRecoveryWorld>,
}

/// 四个告警来源的安全快照；单个来源缺失时 evaluator 会生成 unavailable signal。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationsAlertSnapshot {
    pub process: Option<OperationsAlertProcessSnapshot>,
    pub delivery: Option<OperationsAlertDeliverySnapshot>,
    pub fulfillment: Option<OperationsAlertFulfillmentSnapshot>,
    pub recovery: Option<OperationsAlertRecoverySnapshot>,
}

/// 告警阈值与批次边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertPolicy {
    pub source_timeout: Duration,
    pub batch_window: Duration,
    pub cooldown: Duration,
    pub claim_lease: Duration,
    pub retry_delay: Duration,
    pub max_signals_per_scan: usize,
    pub max_batch_items: usize,
    pub max_body_bytes: usize,
    pub delivery_pending_count_threshold: i64,
    pub delivery_pending_age_threshold: Duration,
    pub delivery_manual_reconcile_count_threshold: i64,
    pub fulfillment_backlog_count_threshold: i64,
    pub fulfillment_backlog_age_threshold: Duration,
    pub recovery_overdue_count_threshold: i64,
    pub recovery_overdue_age_threshold: Duration,
}

impl OperationsAlertPolicy {
    /// 校验时间、数量和正文边界，避免配置扩大外部通知或数据库扫描范围。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.source_timeout < Duration::from_millis(100)
            || self.source_timeout > Duration::from_secs(30)
            || self.batch_window < Duration::from_secs(1)
            || self.batch_window > Duration::from_secs(3_600)
            || self.cooldown < self.batch_window
            || self.cooldown > Duration::from_secs(7 * 24 * 3_600)
            || self.claim_lease < self.source_timeout.saturating_mul(4)
            || self.claim_lease > Duration::from_secs(24 * 3_600)
            || self.retry_delay < Duration::from_secs(1)
            || self.retry_delay > Duration::from_secs(24 * 3_600)
            || self.max_signals_per_scan == 0
            || self.max_signals_per_scan > MAX_OPERATIONS_ALERT_SIGNALS
            || self.max_batch_items == 0
            || self.max_batch_items > MAX_OPERATIONS_ALERT_BATCH_ITEMS
            || self.max_body_bytes < 256
            || self.max_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
            || self.delivery_pending_count_threshold < 1
            || self.delivery_manual_reconcile_count_threshold < 1
            || self.fulfillment_backlog_count_threshold < 1
            || self.recovery_overdue_count_threshold < 1
            || !valid_age_threshold(self.delivery_pending_age_threshold)
            || !valid_age_threshold(self.fulfillment_backlog_age_threshold)
            || !valid_age_threshold(self.recovery_overdue_age_threshold)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}

/// 固定告警收件目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertTarget {
    pub world_id: String,
    pub recipient: String,
}

impl OperationsAlertTarget {
    /// 校验 world 和收件人边界；收件人不会进入 signal fingerprint。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.recipient.is_empty()
            || self.recipient != self.recipient.trim()
            || self.recipient.len() > 256
            || self.recipient.chars().any(char::is_control)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}
