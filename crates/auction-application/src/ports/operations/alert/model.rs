//! operations alert 的来源观察、健康快照和策略模型。

use std::collections::HashSet;

use super::digest::valid_age_threshold;
use super::signals::{OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource};
use super::*;

/// 一次来源观察；不可用时保留固定 `source_unavailable`，并可携带已确认的局部异常。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertSourceObservation {
    pub source: OperationsAlertSource,
    pub available: bool,
    pub signals: Vec<OperationsAlertSignal>,
    /// 本轮没有检查的信号种类（例如主机报告未提供演练目录时的 `restore_drill_overdue`）。
    /// 缺席不等于恢复：存储层关闭“本轮未出现”的活动信号时必须跳过这些种类。
    pub unchecked: Vec<OperationsAlertKind>,
}

impl OperationsAlertSourceObservation {
    pub fn available(
        source: OperationsAlertSource,
        signals: Vec<OperationsAlertSignal>,
    ) -> Result<Self, OperationsAlertError> {
        let result = Self { source, available: true, signals, unchecked: Vec::new() };
        result.validate()?;
        Ok(result)
    }

    /// 标记本轮未检查的种类；该种类的活动信号保持原状，既不刷新也不关闭。
    pub fn with_unchecked(
        mut self,
        kind: OperationsAlertKind,
    ) -> Result<Self, OperationsAlertError> {
        if !self.unchecked.contains(&kind) {
            self.unchecked.push(kind);
        }
        self.validate()?;
        Ok(self)
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
        Ok(Self { source, available: false, signals: vec![signal], unchecked: Vec::new() })
    }

    /// 校验来源完整性与 signal 归属。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.signals.len() > MAX_OPERATIONS_ALERT_SIGNALS {
            return Err(OperationsAlertError::Invalid);
        }
        // 未检查的种类不能同时出现在本轮信号里，也不能豁免固定的来源不可用信号。
        if self.unchecked.iter().any(|kind| {
            *kind == OperationsAlertKind::SourceUnavailable
                || self.signals.iter().any(|signal| signal.kind == *kind)
        }) {
            return Err(OperationsAlertError::Invalid);
        }
        let unavailable_count = self
            .signals
            .iter()
            .filter(|signal| signal.kind == OperationsAlertKind::SourceUnavailable)
            .count();
        if unavailable_count != usize::from(!self.available) {
            return Err(OperationsAlertError::Invalid);
        }
        if self
            .signals
            .iter()
            .any(|signal| signal.validate().is_err() || signal.source != self.source)
        {
            return Err(OperationsAlertError::Invalid);
        }
        // 同一轮观察内按 fingerprint 去重：一个信号只能对应一条持久化水位与一次通知，
        // 重复项必须在进入存储前被拒绝，而不是依赖 upsert 顺序。
        let mut fingerprints = HashSet::with_capacity(self.signals.len());
        if self.signals.iter().any(|signal| !fingerprints.insert(signal.fingerprint)) {
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
    pub returned_mail_manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    pub oldest_returned_mail_age_seconds: i64,
    /// 已领取（claimed/sending）的投递数量；用于识别领取后没有推进的队列延迟。
    pub claimed: i64,
    pub oldest_claimed_age_seconds: i64,
    pub last_update_at: Option<DateTime<Utc>>,
}

/// Settlement fulfillment source 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertFulfillmentSnapshot {
    /// 真正可推进的 ready 履约项；正常等待付款/前置的项不计入。
    pub pending: i64,
    /// 非正常等待的可推进受阻项；失败、已满足及正常等待项不计入。
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    /// 付款期限内等待买家付款的 COD/付款义务；不应作为积压。
    pub waiting_payment: i64,
    /// 等待前置义务完成的卖方货款等；超过策略年龄后才应视为积压。
    pub waiting_prerequisite: i64,
    pub oldest_backlog_age_seconds: i64,
    pub oldest_prerequisite_wait_age_seconds: i64,
}

/// Recovery 每个 world 的余额和 evidence 安全状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    pub evidence_age_seconds: i64,
    /// 余额读取连续处于同一非正常状态（busy/unavailable）的持续时间；正常时为 0。
    pub balance_stall_age_seconds: i64,
    pub balance_observed_at: Option<DateTime<Utc>>,
    pub evidence_last_success_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Recovery source 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertRecoverySnapshot {
    pub payment_overdue_count: i64,
    /// 付款期限已过、付款投递没有自动或人工发送证明的恢复数量。
    pub payment_mail_unsent_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
    pub oldest_payment_mail_unsent_age_seconds: i64,
    pub worlds: Vec<OperationsAlertRecoveryWorld>,
}

/// Mail source 快照：收件轮询水位。Admin 显式暂停的邮箱由读取层排除，不进入该水位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertMailSnapshot {
    pub last_poll_age_seconds: i64,
    pub consecutive_failures: i64,
    /// 最近一次完整 Sent 枚举扫描的页数；None 表示尚无完整枚举观察。
    pub sent_pages_scanned: Option<i64>,
    /// 该次完整枚举生效的页数上限；与 `sent_pages_scanned` 必须同时存在。
    pub sent_page_limit: Option<i64>,
}

/// Credential source 快照：每种凭据当前行的失效数量。
/// 只用稳定种类名，不携带凭据标签、指纹或密文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertCredentialSnapshot {
    pub invalid_mail_cookie: i64,
    pub invalid_external_api: i64,
}

/// Lifecycle source 快照：结束时间已过但尚未推进的场次与慢速代理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertLifecycleSnapshot {
    pub phase_overdue_count: i64,
    pub oldest_phase_overdue_age_seconds: i64,
    pub proxy_expiry_overdue_count: i64,
    pub oldest_proxy_overdue_age_seconds: i64,
    /// 冻结 isekai 换季边界前七天内仍未完成的结算义务数。
    pub season_obligations_pending: i64,
}

/// Forum source 快照：主题发布与私信投递失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertForumSnapshot {
    pub sync_failed_count: i64,
    pub oldest_sync_failed_age_seconds: i64,
    pub message_failed_count: i64,
    pub oldest_message_failed_age_seconds: i64,
    /// 最近一次远端主题扫描发现的最大 HTTP 分页数；不是本地目录固定帖页数。
    pub remote_page_count: Option<i64>,
    /// 该次观察生效的远端主题页数上限；与 `remote_page_count` 必须同时存在。
    pub remote_page_limit: Option<i64>,
}

/// Notification source 快照：非结算通知投递失败与邮资不足。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertNotificationSnapshot {
    pub failed_count: i64,
    pub oldest_failed_age_seconds: i64,
    pub postage_insufficient_count: i64,
    pub oldest_postage_age_seconds: i64,
}

/// W11.1–W11.3 资产对账在单个 world 的最新报告事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertReconciliationWorld {
    pub world: &'static str,
    /// 最近一份 custody 报告的缺失装备数量；报告缺失时为 0，由报告年龄表达缺失。
    pub custody_missing_count: i64,
    /// 最近一份 solvency 报告的缺口 credits；报告缺失或余额未知时为 0。
    pub solvency_shortfall_credits: i64,
}

/// 资产对账报告投影；`None` 表示该类报告从未追加，不能当作“无差异”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsAlertReconciliationSnapshot {
    pub custody_report_age_seconds: Option<i64>,
    pub solvency_report_age_seconds: Option<i64>,
    pub worlds: Vec<OperationsAlertReconciliationWorld>,
}

/// 全部告警来源的安全快照；单个来源缺失时 evaluator 会生成 unavailable signal。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationsAlertSnapshot {
    pub process: Option<OperationsAlertProcessSnapshot>,
    pub delivery: Option<OperationsAlertDeliverySnapshot>,
    pub fulfillment: Option<OperationsAlertFulfillmentSnapshot>,
    pub recovery: Option<OperationsAlertRecoverySnapshot>,
    pub mail: Option<OperationsAlertMailSnapshot>,
    pub credential: Option<OperationsAlertCredentialSnapshot>,
    pub lifecycle: Option<OperationsAlertLifecycleSnapshot>,
    pub forum: Option<OperationsAlertForumSnapshot>,
    pub notification: Option<OperationsAlertNotificationSnapshot>,
    pub reconciliation: Option<OperationsAlertReconciliationSnapshot>,
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
            || !auction_domain::valid_text(&self.recipient, 256)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }
}
