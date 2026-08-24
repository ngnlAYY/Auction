//! operations alert 的领域安全投影、阈值规则和窄存储端口。
//!
//! 告警不是拍卖事实，也不能改变竞价、结算或履约结果。该模块只把已经由各 owner 提供的
//! 健康摘要转换成稳定 signal，并通过独立端口交给 PostgreSQL 完整档保存。通知入队仍然
//! 发生在扫描事务之后，避免外部 MoogleMail 结果反向决定告警事实。

use std::{fmt, time::Duration};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use super::{ApplicationFuture, RequestHash};
use crate::ApplicationError;

/// 单轮最多允许冻结的告警 signal 数量。
pub const MAX_OPERATIONS_ALERT_SIGNALS: usize = 64;
/// 单个告警批次最多包含的 signal 数量。
pub const MAX_OPERATIONS_ALERT_BATCH_ITEMS: usize = 64;
/// 告警批次正文的最大字节数。
pub const MAX_OPERATIONS_ALERT_BODY_BYTES: usize = 16 << 10;

/// 告警来源闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSource {
    Process,
    Delivery,
    Fulfillment,
    Recovery,
}

impl OperationsAlertSource {
    /// 返回数据库和日志使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Delivery => "delivery",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
        }
    }
}

impl fmt::Display for OperationsAlertSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 持久化告警类型闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertKind {
    ServiceMissing,
    ServiceDegraded,
    AuditFailure,
    SourceUnavailable,
    DeliveryPending,
    DeliveryManualReconcile,
    FulfillmentBacklog,
    FulfillmentFailed,
    FulfillmentManualReconcile,
    RecoveryPaymentOverdue,
    RecoveryRelistPending,
    RecoveryFailed,
    RecoveryManualReconcile,
    BalanceLow,
    EvidenceStale,
}

impl OperationsAlertKind {
    /// 返回数据库和通知摘要使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServiceMissing => "service_missing",
            Self::ServiceDegraded => "service_degraded",
            Self::AuditFailure => "audit_failure",
            Self::SourceUnavailable => "source_unavailable",
            Self::DeliveryPending => "delivery_pending",
            Self::DeliveryManualReconcile => "delivery_manual_reconcile",
            Self::FulfillmentBacklog => "fulfillment_backlog",
            Self::FulfillmentFailed => "fulfillment_failed",
            Self::FulfillmentManualReconcile => "fulfillment_manual_reconcile",
            Self::RecoveryPaymentOverdue => "recovery_payment_overdue",
            Self::RecoveryRelistPending => "recovery_relist_pending",
            Self::RecoveryFailed => "recovery_failed",
            Self::RecoveryManualReconcile => "recovery_manual_reconcile",
            Self::BalanceLow => "balance_low",
            Self::EvidenceStale => "evidence_stale",
        }
    }
}

impl fmt::Display for OperationsAlertKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// operations alert 应用层错误；错误正文不能携带原始外部响应或收件人内容。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertError {
    #[error("operations alert 参数无效")]
    Invalid,
    #[error("operations alert 来源不可用")]
    SourceUnavailable,
    #[error("operations alert 持久化结果无效")]
    StoredResultCorrupt,
    #[error("operations alert claim 已过期或 fencing 不匹配")]
    StaleClaim,
    #[error("operations alert 依赖身份不匹配")]
    IdentityConflict,
}

/// 一条稳定、无敏感内容的告警 signal。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationsAlertSignal {
    pub fingerprint: [u8; 32],
    pub kind: OperationsAlertKind,
    pub scope: String,
    pub source: OperationsAlertSource,
    pub count: i64,
    pub age_seconds: i64,
}

impl OperationsAlertSignal {
    /// 按 kind/scope 生成稳定 fingerprint，并校验 source 归属闭集。
    pub fn new(
        kind: OperationsAlertKind,
        scope: impl Into<String>,
        source: OperationsAlertSource,
        count: i64,
        age_seconds: i64,
    ) -> Result<Self, OperationsAlertError> {
        let scope = scope.into();
        let signal = Self {
            fingerprint: signal_fingerprint(kind, &scope),
            kind,
            scope,
            source,
            count,
            age_seconds,
        };
        signal.validate()?;
        Ok(signal)
    }

    /// 校验持久化前后的 signal identity 与数值边界。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.count < 0
            || self.age_seconds < 0
            || self.scope.is_empty()
            || self.scope.len() > 128
            || self.scope != self.scope.trim()
            || self.scope.chars().any(char::is_control)
            || self.fingerprint != signal_fingerprint(self.kind, &self.scope)
            || !valid_signal_identity(self.kind, &self.scope, self.source)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }

    /// 生成不含 ID、收件人、原始错误或凭据的单行通知摘要。
    pub fn render_summary(&self) -> Result<String, OperationsAlertError> {
        self.validate()?;
        let summary = match self.kind {
            OperationsAlertKind::ServiceMissing => format!("{} 服务缺失", self.scope),
            OperationsAlertKind::ServiceDegraded => format!("{} 服务降级", self.scope),
            OperationsAlertKind::AuditFailure => format!("审计失败累计 {}", self.count),
            OperationsAlertKind::SourceUnavailable => format!("{} 健康来源不可用", self.scope),
            OperationsAlertKind::DeliveryPending => {
                format!("交付积压 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::DeliveryManualReconcile => {
                format!("交付人工对账 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::FulfillmentBacklog => {
                format!("结算履约积压 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::FulfillmentFailed => format!("结算履约失败 {}", self.count),
            OperationsAlertKind::FulfillmentManualReconcile => {
                format!("结算履约人工对账 {}", self.count)
            }
            OperationsAlertKind::RecoveryPaymentOverdue => {
                format!("结算付款逾期 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::RecoveryRelistPending => format!("结算恢复待重挂 {}", self.count),
            OperationsAlertKind::RecoveryFailed => format!("结算恢复失败 {}", self.count),
            OperationsAlertKind::RecoveryManualReconcile => {
                format!("结算恢复人工对账 {}", self.count)
            }
            OperationsAlertKind::BalanceLow => {
                format!("{} 结算余额偏低，当前 {}", self.scope, self.count)
            }
            OperationsAlertKind::EvidenceStale => {
                format!("{} 恢复证据过期 {} 秒", self.scope, self.age_seconds)
            }
        };
        if summary.is_empty()
            || summary != summary.trim()
            || summary.len() > 512
            || summary.chars().any(char::is_control)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(summary)
    }
}

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

/// 从四个 source 快照生成完整 signal 集合。
pub fn evaluate_snapshot(
    snapshot: &OperationsAlertSnapshot,
    policy: &OperationsAlertPolicy,
) -> Result<Vec<OperationsAlertSourceObservation>, OperationsAlertError> {
    policy.validate()?;
    let result = vec![
        evaluate_process(snapshot.process.as_ref())?,
        evaluate_delivery(snapshot.delivery, policy)?,
        evaluate_fulfillment(snapshot.fulfillment, policy)?,
        evaluate_recovery(snapshot.recovery.as_ref(), policy)?,
    ];
    let total = result.iter().map(|observation| observation.signals.len()).sum::<usize>();
    if total > policy.max_signals_per_scan {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

fn evaluate_process(
    snapshot: Option<&OperationsAlertProcessSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Process);
    };
    if snapshot.audit_failure_count < 0 || snapshot.processes.len() != 3 {
        return Err(OperationsAlertError::Invalid);
    }
    let mut seen = [false; 3];
    let mut signals = Vec::new();
    for process in &snapshot.processes {
        let index = match process.role.as_str() {
            "public" => 0,
            "admin" => 1,
            "worker" => 2,
            _ => return Err(OperationsAlertError::Invalid),
        };
        if seen[index] {
            return Err(OperationsAlertError::Invalid);
        }
        seen[index] = true;
        let kind = match process.status.as_str() {
            "ready" => None,
            "missing" => Some(OperationsAlertKind::ServiceMissing),
            "degraded" => Some(OperationsAlertKind::ServiceDegraded),
            _ => return Err(OperationsAlertError::Invalid),
        };
        if let Some(kind) = kind {
            signals.push(OperationsAlertSignal::new(
                kind,
                process.role.clone(),
                OperationsAlertSource::Process,
                1,
                0,
            )?);
        }
    }
    if !seen.into_iter().all(|value| value) {
        return Err(OperationsAlertError::Invalid);
    }
    if snapshot.audit_failure_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::AuditFailure,
            "audit",
            OperationsAlertSource::Process,
            snapshot.audit_failure_count,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Process, signals)
}

fn evaluate_delivery(
    snapshot: Option<OperationsAlertDeliverySnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Delivery);
    };
    if snapshot.pending < 0
        || snapshot.failed < 0
        || snapshot.manual_reconcile < 0
        || snapshot.oldest_pending_age_seconds < 0
        || snapshot.oldest_manual_reconcile_age_seconds < 0
        || (snapshot.pending == 0 && snapshot.oldest_pending_age_seconds != 0)
        || (snapshot.manual_reconcile == 0 && snapshot.oldest_manual_reconcile_age_seconds != 0)
        || snapshot.last_update_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.pending >= policy.delivery_pending_count_threshold
        || snapshot.oldest_pending_age_seconds
            >= policy.delivery_pending_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryPending,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.pending,
            snapshot.oldest_pending_age_seconds,
        )?);
    }
    if snapshot.manual_reconcile >= policy.delivery_manual_reconcile_count_threshold {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryManualReconcile,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.manual_reconcile,
            snapshot.oldest_manual_reconcile_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Delivery, signals)
}

fn evaluate_fulfillment(
    snapshot: Option<OperationsAlertFulfillmentSnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Fulfillment);
    };
    let backlog =
        snapshot.pending.checked_add(snapshot.blocked).ok_or(OperationsAlertError::Invalid)?;
    if snapshot.pending < 0
        || snapshot.blocked < 0
        || snapshot.failed < 0
        || snapshot.manual_reconcile < 0
        || snapshot.oldest_backlog_age_seconds < 0
        || (backlog == 0 && snapshot.oldest_backlog_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if backlog >= policy.fulfillment_backlog_count_threshold
        || snapshot.oldest_backlog_age_seconds
            >= policy.fulfillment_backlog_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentBacklog,
            "auction",
            OperationsAlertSource::Fulfillment,
            backlog,
            snapshot.oldest_backlog_age_seconds,
        )?);
    }
    if snapshot.failed > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentFailed,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.failed,
            0,
        )?);
    }
    if snapshot.manual_reconcile > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentManualReconcile,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.manual_reconcile,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Fulfillment, signals)
}

fn evaluate_recovery(
    snapshot: Option<&OperationsAlertRecoverySnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Recovery);
    };
    if snapshot.payment_overdue_count < 0
        || snapshot.relist_pending_count < 0
        || snapshot.failed_action_count < 0
        || snapshot.manual_reconcile_count < 0
        || snapshot.oldest_overdue_age_seconds < 0
        || (snapshot.payment_overdue_count == 0 && snapshot.oldest_overdue_age_seconds != 0)
        || snapshot.worlds.is_empty()
        || snapshot.worlds.len() > 2
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.payment_overdue_count >= policy.recovery_overdue_count_threshold
        || snapshot.oldest_overdue_age_seconds
            >= policy.recovery_overdue_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryPaymentOverdue,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.payment_overdue_count,
            snapshot.oldest_overdue_age_seconds,
        )?);
    }
    if snapshot.relist_pending_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryRelistPending,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.relist_pending_count,
            0,
        )?);
    }
    if snapshot.failed_action_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryFailed,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.failed_action_count,
            0,
        )?);
    }
    if snapshot.manual_reconcile_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryManualReconcile,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.manual_reconcile_count,
            0,
        )?);
    }
    let mut worlds = Vec::new();
    for world in &snapshot.worlds {
        if !matches!(world.world.as_str(), "main" | "isekai")
            || worlds.iter().any(|value: &String| value == &world.world)
            || world.balance_credits < 0
            || world.evidence_age_seconds < 0
            || world.updated_at.timestamp_nanos_opt().is_none()
            || world.balance_observed_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || world
                .evidence_last_success_at
                .is_some_and(|value| value.timestamp_nanos_opt().is_none())
        {
            return Err(OperationsAlertError::Invalid);
        }
        worlds.push(world.world.clone());
        match world.balance_status.as_str() {
            "ok" => {}
            "low" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::BalanceLow,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                world.balance_credits,
                0,
            )?),
            "unknown" | "stale" | "unavailable" => {
                return OperationsAlertSourceObservation::unavailable(
                    OperationsAlertSource::Recovery,
                );
            }
            _ => return Err(OperationsAlertError::Invalid),
        }
        match world.evidence_status.as_str() {
            "fresh" => {}
            "stale" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::EvidenceStale,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                1,
                world.evidence_age_seconds,
            )?),
            "unknown" | "unavailable" => {
                return OperationsAlertSourceObservation::unavailable(
                    OperationsAlertSource::Recovery,
                );
            }
            _ => return Err(OperationsAlertError::Invalid),
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Recovery, signals)
}

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
        for (observation, source) in self.observations.iter().zip(expected) {
            if observation.source != source {
                return Err(OperationsAlertError::Invalid);
            }
            observation.validate()?;
        }
        Ok(())
    }
}

/// 领取 due 告警批次的请求。
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
                || item.ordinal != i32::try_from(index + 1).unwrap_or_default()
                || item.summary.is_empty()
                || item.summary.len() > 512
                || item.summary != item.summary.trim()
                || item.summary.chars().any(char::is_control)
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

/// 已入队批次的最小对账投影。
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
    /// 返回数据库状态名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ManualReconcile => "manual_reconcile",
        }
    }
}

/// delivery 创建阶段允许持久化的稳定失败类别。
///
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

/// 生成批次正文的稳定单行/多行表示。
pub fn canonical_summary(
    items: &[OperationsAlertBatchItem],
    maximum_body_bytes: usize,
) -> Result<String, OperationsAlertError> {
    if items.is_empty()
        || items.len() > MAX_OPERATIONS_ALERT_BATCH_ITEMS
        || maximum_body_bytes == 0
        || maximum_body_bytes > MAX_OPERATIONS_ALERT_BODY_BYTES
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut lines = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if item.ordinal != i32::try_from(index + 1).unwrap_or_default()
            || item.fingerprint == [0; 32]
            || item.signal_version <= 0
            || item.summary.is_empty()
            || item.summary != item.summary.trim()
            || item.summary.len() > 512
            || item.summary.chars().any(char::is_control)
        {
            return Err(OperationsAlertError::StoredResultCorrupt);
        }
        lines.push(format!("- {}", item.summary));
    }
    let result = lines.join("\n");
    if result.len() > maximum_body_bytes {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

/// 计算冻结正文摘要。
pub fn body_digest(summary: &str) -> [u8; 32] {
    Sha256::digest(summary.as_bytes()).into()
}

/// 计算 operations alert delivery 使用的稳定请求摘要。
pub fn request_hash(
    batch_id: Uuid,
    delivery_id: Uuid,
    command_id: Uuid,
    target: &OperationsAlertTarget,
    summary: &str,
) -> Result<RequestHash, ApplicationError> {
    #[derive(Serialize)]
    struct Request<'a> {
        batch_id: Uuid,
        delivery_id: Uuid,
        command_id: Uuid,
        world_id: &'a str,
        recipient: &'a str,
        summary: &'a str,
    }
    target.validate().map_err(ApplicationError::OperationsAlert)?;
    if summary.is_empty() || summary.len() > MAX_OPERATIONS_ALERT_BODY_BYTES {
        return Err(OperationsAlertError::Invalid.into());
    }
    RequestHash::from_request(
        "operations_alert_delivery",
        batch_id,
        &Request {
            batch_id,
            delivery_id,
            command_id,
            world_id: &target.world_id,
            recipient: &target.recipient,
            summary,
        },
    )
    .map_err(ApplicationError::from)
}

fn signal_fingerprint(kind: OperationsAlertKind, scope: &str) -> [u8; 32] {
    Sha256::digest(format!("operations-alert:v1\0{}\0{scope}", kind.as_str()).as_bytes()).into()
}

fn valid_signal_identity(
    kind: OperationsAlertKind,
    scope: &str,
    source: OperationsAlertSource,
) -> bool {
    match kind {
        OperationsAlertKind::ServiceMissing | OperationsAlertKind::ServiceDegraded => {
            source == OperationsAlertSource::Process
                && matches!(scope, "public" | "admin" | "worker")
        }
        OperationsAlertKind::AuditFailure => {
            source == OperationsAlertSource::Process && scope == "audit"
        }
        OperationsAlertKind::SourceUnavailable => scope == source.as_str(),
        OperationsAlertKind::DeliveryPending | OperationsAlertKind::DeliveryManualReconcile => {
            source == OperationsAlertSource::Delivery && scope == "delivery"
        }
        OperationsAlertKind::FulfillmentBacklog
        | OperationsAlertKind::FulfillmentFailed
        | OperationsAlertKind::FulfillmentManualReconcile => {
            source == OperationsAlertSource::Fulfillment && scope == "auction"
        }
        OperationsAlertKind::RecoveryPaymentOverdue
        | OperationsAlertKind::RecoveryRelistPending
        | OperationsAlertKind::RecoveryFailed
        | OperationsAlertKind::RecoveryManualReconcile => {
            source == OperationsAlertSource::Recovery && scope == "auction"
        }
        OperationsAlertKind::BalanceLow | OperationsAlertKind::EvidenceStale => {
            source == OperationsAlertSource::Recovery && matches!(scope, "main" | "isekai")
        }
    }
}

fn valid_age_threshold(value: Duration) -> bool {
    value >= Duration::from_secs(1)
        && value <= Duration::from_secs(30 * 24 * 3_600)
        && value.subsec_nanos() == 0
}

fn valid_health_error_code(value: &str) -> bool {
    matches!(
        value,
        "" | "timeout"
            | "canceled"
            | "persistence"
            | "dependency_mismatch"
            | "delivery_failed"
            | "delivery_manual_reconcile"
            | "retry_exhausted"
    )
}
