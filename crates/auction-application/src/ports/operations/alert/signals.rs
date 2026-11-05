//! operations alert 来源、signal 类型和安全错误。

use super::digest::{signal_fingerprint, valid_signal_identity};
use super::*;

/// 告警来源闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSource {
    Process,
    Delivery,
    Fulfillment,
    Recovery,
}

impl OperationsAlertSource {
    /// 来源闭集；与迁移 005/020 的 `operations_alert_signals.source` CHECK 一致。
    pub const ALL: [Self; 4] = [Self::Process, Self::Delivery, Self::Fulfillment, Self::Recovery];

    /// 返回数据库和日志使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Delivery => "delivery",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
        }
    }

    /// 从持久化名称解析；未知名称返回 `None`，调用方不得回退到默认来源。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == value)
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
    DeliveryReturnedMail,
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
    /// 持久化种类闭集；与迁移 005/020 的 `operations_alert_signals.kind` CHECK 一致。
    pub const ALL: [Self; 16] = [
        Self::ServiceMissing,
        Self::ServiceDegraded,
        Self::AuditFailure,
        Self::SourceUnavailable,
        Self::DeliveryPending,
        Self::DeliveryReturnedMail,
        Self::DeliveryManualReconcile,
        Self::FulfillmentBacklog,
        Self::FulfillmentFailed,
        Self::FulfillmentManualReconcile,
        Self::RecoveryPaymentOverdue,
        Self::RecoveryRelistPending,
        Self::RecoveryFailed,
        Self::RecoveryManualReconcile,
        Self::BalanceLow,
        Self::EvidenceStale,
    ];

    /// 返回数据库和通知摘要使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServiceMissing => "service_missing",
            Self::ServiceDegraded => "service_degraded",
            Self::AuditFailure => "audit_failure",
            Self::SourceUnavailable => "source_unavailable",
            Self::DeliveryPending => "delivery_pending",
            Self::DeliveryReturnedMail => "delivery_returned_mail",
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

    /// 告警分级（决策 66）：只有付款逾期与待重列属于非紧急的每日摘要。
    pub const fn severity(self) -> OperationsAlertSeverity {
        match self {
            Self::RecoveryPaymentOverdue | Self::RecoveryRelistPending => {
                OperationsAlertSeverity::Digest
            }
            _ => OperationsAlertSeverity::Urgent,
        }
    }

    /// 该种类下一次重复通知的最小间隔；紧急信号沿用策略冷却，非紧急信号不短于每日摘要节奏。
    pub fn repeat_interval(self, cooldown: Duration) -> Duration {
        match self.severity() {
            OperationsAlertSeverity::Urgent => cooldown,
            OperationsAlertSeverity::Digest => cooldown.max(OPERATIONS_ALERT_DIGEST_INTERVAL),
        }
    }
}

impl fmt::Display for OperationsAlertKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 非紧急（每日摘要）信号的重复通知节奏。
pub const OPERATIONS_ALERT_DIGEST_INTERVAL: Duration = Duration::from_secs(24 * 3_600);

/// 告警分级闭集（决策 66）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSeverity {
    /// 需要立即处理；按策略冷却重复通知。
    Urgent,
    /// 非紧急；重复通知并入每日摘要节奏（摘要聚合本身属于 W5.8）。
    Digest,
}

impl OperationsAlertSeverity {
    /// 返回稳定名称；日志与后续告警台使用同一闭集。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::Digest => "digest",
        }
    }
}

impl fmt::Display for OperationsAlertSeverity {
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

    /// 返回该 signal 的告警分级。
    pub const fn severity(&self) -> OperationsAlertSeverity {
        self.kind.severity()
    }

    /// 返回该 signal 下一次重复通知的最小间隔。
    pub fn repeat_interval(&self, cooldown: Duration) -> Duration {
        self.kind.repeat_interval(cooldown)
    }

    /// 校验持久化前后的 signal identity 与数值边界。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.count < 0
            || self.age_seconds < 0
            || !auction_domain::valid_text(&self.scope, 128)
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
            OperationsAlertKind::DeliveryReturnedMail => format!(
                "MoogleMail 退信待取回 {}，最长 {} 秒；请核对后台交付页和 MM",
                self.count, self.age_seconds
            ),
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
        if !auction_domain::valid_text(&summary, 512) {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(summary)
    }
}
