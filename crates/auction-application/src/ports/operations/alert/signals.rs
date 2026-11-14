//! operations alert 来源、signal 类型和安全错误。

use super::digest::{signal_fingerprint, valid_signal_identity};
use serde::Serialize;
use std::fmt;
use std::time::Duration;
use thiserror::Error;

/// 告警来源闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSource {
    Process,
    Delivery,
    Fulfillment,
    Recovery,
    /// 收件轮询（MoogleMail 收件扫描）停滞。
    Mail,
    /// 外部凭据失效；同一凭据种类只有一个当前行。
    Credential,
    /// 生命周期与代理到期推进逾期。
    Lifecycle,
    /// 论坛发布与私信投递失败。
    Forum,
    /// 非结算通知投递失败（含邮资不足）。
    Notification,
    /// 主机侧备份证据：备份目录容量与最近一次定时备份终态。
    /// 由 `auction-cli alert report` 在主机上读取回执与文件系统，不进入 Worker 扫描。
    Backup,
    /// 主机侧 WAL 归档证据：暂存目录里超过时限仍未上传的段。
    /// 同样由 `auction-cli alert report` 单独观察。
    Wal,
    /// 资产对账：最近追加的装备持有与偿付能力报告。
    Reconciliation,
}

impl OperationsAlertSource {
    /// Worker 每轮扫描必须提交的来源闭集，顺序即 `OperationsAlertObserveCommand` 的校验顺序。
    ///
    /// 主机侧来源不由 Worker 观察：Worker 无法读取主机文件系统，伪造一条“来源不可用”会
    /// 掩盖真实状态；它们由 CLI 单独提交 `OperationsAlertHostObserveCommand`。
    pub const WORKER: [Self; 10] = [
        Self::Process,
        Self::Delivery,
        Self::Fulfillment,
        Self::Recovery,
        Self::Mail,
        Self::Credential,
        Self::Lifecycle,
        Self::Forum,
        Self::Notification,
        Self::Reconciliation,
    ];

    /// 主机侧来源闭集，顺序即 `OperationsAlertHostObserveCommand` 的校验顺序。
    pub const HOST: [Self; 2] = [Self::Backup, Self::Wal];

    /// 持久化来源闭集（Worker 扫描 + 主机侧报告）；与迁移 005/020/085/091 的
    /// `operations_alert_signals.source` CHECK 一致。
    pub const ALL: [Self; 12] = [
        Self::Process,
        Self::Delivery,
        Self::Fulfillment,
        Self::Recovery,
        Self::Mail,
        Self::Credential,
        Self::Lifecycle,
        Self::Forum,
        Self::Notification,
        Self::Backup,
        Self::Wal,
        Self::Reconciliation,
    ];

    /// 返回数据库和日志使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Delivery => "delivery",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
            Self::Mail => "mail",
            Self::Credential => "credential",
            Self::Lifecycle => "lifecycle",
            Self::Forum => "forum",
            Self::Notification => "notification",
            Self::Backup => "backup",
            Self::Wal => "wal",
            Self::Reconciliation => "reconciliation",
        }
    }

    /// 从持久化名称解析；未知名称返回 `None`，调用方不得回退到默认来源。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == value)
    }

    /// 处置手册锚点：在 `ops/README.md` 为每个来源固定 `<a id>`，告警台直接跳转。
    pub const fn runbook_anchor(self) -> &'static str {
        match self {
            Self::Process => "alert-runbook-process",
            Self::Delivery => "alert-runbook-delivery",
            Self::Fulfillment => "alert-runbook-fulfillment",
            Self::Recovery => "alert-runbook-recovery",
            Self::Mail => "alert-runbook-mail",
            Self::Credential => "alert-runbook-credential",
            Self::Lifecycle => "alert-runbook-lifecycle",
            Self::Forum => "alert-runbook-forum",
            Self::Notification => "alert-runbook-notification",
            Self::Backup => "alert-runbook-backup",
            Self::Wal => "alert-runbook-wal",
            Self::Reconciliation => "alert-runbook-reconciliation",
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
    DeliveryReturnedMail,
    DeliveryManualReconcile,
    FulfillmentBacklog,
    FulfillmentFailed,
    FulfillmentManualReconcile,
    RecoveryPaymentOverdue,
    /// 付款期限已过，但对应付款投递没有自动或人工发送证明；提醒已延后，需人工核对。
    RecoveryPaymentMailUnsent,
    RecoveryRelistPending,
    RecoveryFailed,
    RecoveryManualReconcile,
    BalanceLow,
    EvidenceStale,
    /// 收件轮询停滞：本轮次心跳水位超过阈值，或连续失败次数非零。
    MailPollStalled,
    /// Sent 最近一次完整枚举已达到配置页数上限的 80%。
    MailSentPageLimitApproaching,
    /// 当前凭据行失效（`status = 'invalid'`），该凭据种类没有可用行。
    CredentialInvalid,
    /// 场次结束时间已过但仍在 live，没有进入 Final。
    AuctionPhaseOverdue,
    /// 慢速代理到期时间已过仍未执行。
    ProxyExpiryOverdue,
    /// 已领取但长时间没有推进的投递（队列延迟）。
    QueueLatency,
    /// 论坛主题发布失败。
    ForumSyncFailed,
    /// 论坛私信投递进入人工对账（发送失败）。
    ForumMessageFailed,
    /// 论坛主题远端 HTTP 分页已达到配置上限的 80%。
    ForumTopicPageLimitApproaching,
    /// 非结算通知投递失败。
    NotificationDeliveryFailed,
    /// 非结算通知因邮资不足被远端拒绝。
    NotificationPostageInsufficient,
    /// 外部游戏账号长时间处于忙或不可用状态。
    AccountBusy,
    /// 备份目录所在文件系统低于配置的字节与比例水位。
    DiskCapacityLow,
    /// 最近一次定时备份终态为失败或中断。
    BackupFailed,
    /// 决策 31：超过固定时限没有成功的恢复演练（恢复回执或通过的迁移演练），或从未记录过。
    RestoreDrillOverdue,
    /// WAL 暂存目录里存在超过固定时限仍未上传的段：归档/上传链路积压。
    WalUploadStalled,
    /// 冻结的 isekai 换季边界前仍有未完成义务。
    SeasonObligationsPending,
    /// 装备持有与数据库应持集合存在差异。
    CustodyDrift,
    /// 待付货款/退款/邮资超过最近一次新鲜余额。
    SolvencyShortfall,
    /// 报告从未追加或超过固定时限未更新，对账视野已失效。
    ReconciliationStale,
}

impl OperationsAlertKind {
    /// 持久化种类闭集；与迁移 005/020/085/091/095/099/101/104 的 `operations_alert_signals.kind`
    /// CHECK 一致。
    pub const ALL: [Self; 37] = [
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
        Self::RecoveryPaymentMailUnsent,
        Self::RecoveryRelistPending,
        Self::RecoveryFailed,
        Self::RecoveryManualReconcile,
        Self::BalanceLow,
        Self::EvidenceStale,
        Self::MailPollStalled,
        Self::MailSentPageLimitApproaching,
        Self::CredentialInvalid,
        Self::AuctionPhaseOverdue,
        Self::ProxyExpiryOverdue,
        Self::QueueLatency,
        Self::ForumSyncFailed,
        Self::ForumMessageFailed,
        Self::ForumTopicPageLimitApproaching,
        Self::NotificationDeliveryFailed,
        Self::NotificationPostageInsufficient,
        Self::AccountBusy,
        Self::DiskCapacityLow,
        Self::BackupFailed,
        Self::RestoreDrillOverdue,
        Self::WalUploadStalled,
        Self::SeasonObligationsPending,
        Self::CustodyDrift,
        Self::SolvencyShortfall,
        Self::ReconciliationStale,
    ];

    /// 从持久化名称解析全部已登记种类；未知名称返回 `None`，调用方不得回退到默认种类。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

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
            Self::RecoveryPaymentMailUnsent => "recovery_payment_mail_unsent",
            Self::RecoveryRelistPending => "recovery_relist_pending",
            Self::RecoveryFailed => "recovery_failed",
            Self::RecoveryManualReconcile => "recovery_manual_reconcile",
            Self::BalanceLow => "balance_low",
            Self::EvidenceStale => "evidence_stale",
            Self::MailPollStalled => "mail_poll_stalled",
            Self::MailSentPageLimitApproaching => "mail_sent_page_limit_approaching",
            Self::CredentialInvalid => "credential_invalid",
            Self::AuctionPhaseOverdue => "auction_phase_overdue",
            Self::ProxyExpiryOverdue => "proxy_expiry_overdue",
            Self::QueueLatency => "queue_latency",
            Self::ForumSyncFailed => "forum_sync_failed",
            Self::ForumMessageFailed => "forum_message_failed",
            Self::ForumTopicPageLimitApproaching => "forum_topic_page_limit_approaching",
            Self::NotificationDeliveryFailed => "notification_delivery_failed",
            Self::NotificationPostageInsufficient => "notification_postage_insufficient",
            Self::AccountBusy => "account_busy",
            Self::DiskCapacityLow => "disk_capacity_low",
            Self::BackupFailed => "backup_failed",
            Self::RestoreDrillOverdue => "restore_drill_overdue",
            Self::WalUploadStalled => "wal_upload_stalled",
            Self::SeasonObligationsPending => "season_obligations_pending",
            Self::CustodyDrift => "custody_drift",
            Self::SolvencyShortfall => "solvency_shortfall",
            Self::ReconciliationStale => "reconciliation_stale",
        }
    }

    /// 告警分级（决策 66）：付款逾期、待重列等自动恢复的等待类信号属于每日摘要；
    /// 需要立即人工介入的信号按策略冷却重复通知。
    /// 论坛发布、非结算通知失败与队列延迟都会由 Worker 自动重试或推进，重复通知只会放大
    /// 同一故障的噪声，因此并入每日摘要；邮资不足、账号忙、凭据失效与轮询停滞不会自愈。
    /// 主机侧的磁盘水位、备份失败与 WAL 积压同样不会自愈，且会阻塞 PostgreSQL 回收，
    /// 保持紧急级；它们只由 CLI 在固定观察窗口后上报，不会因一次瞬时抖动反复通知。
    pub const fn severity(self) -> OperationsAlertSeverity {
        match self {
            Self::RecoveryPaymentOverdue
            | Self::RecoveryRelistPending
            | Self::QueueLatency
            | Self::ForumSyncFailed
            | Self::NotificationDeliveryFailed
            // 演练逾期不影响当前服务，按每日摘要提醒即可。
            | Self::RestoreDrillOverdue => OperationsAlertSeverity::Digest,
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

/// 新增来源的固定最小观察窗口。它们不进入运行配置：阈值只决定“何时算异常”，
/// 放宽或收紧都需要新的代码审阅，避免运维配置扩大一遍扫描的告警面。
/// 收件轮询停滞：最后一次成功轮询水位超过该年龄即告警。
pub const OPERATIONS_ALERT_MAIL_POLL_STALL_AGE: Duration = Duration::from_secs(15 * 60);
/// 生命周期逾期宽限：场次结束或慢速代理到期后仍允许 Worker 推进的窗口。
pub const OPERATIONS_ALERT_LIFECYCLE_GRACE: Duration = Duration::from_secs(15 * 60);
/// 队列延迟：已被领取但超过该年龄仍未推进的投递。
pub const OPERATIONS_ALERT_QUEUE_LATENCY_AGE: Duration = Duration::from_secs(10 * 60);
/// 外部账号持续忙：同一世界余额读取连续处于忙状态的持续时间。
pub const OPERATIONS_ALERT_ACCOUNT_BUSY_AGE: Duration = Duration::from_secs(15 * 60);
/// WAL 暂存积压：`archive_timeout` 为 300 秒、上传器每 5 分钟运行一次，因此一段暂存
/// 超过 30 分钟仍未上传说明归档或上传链路已持续失败。该阈值不进入运行配置。
pub const OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE: Duration = Duration::from_secs(30 * 60);
/// 决策 31 要求每月一次恢复演练；最近一次成功演练超过 35 天（月度加 5 天余量）即逾期。
/// 该阈值不进入运行配置。
pub const OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE: Duration = Duration::from_secs(35 * 24 * 3_600);
/// 资产对账报告过期：固定对账周期为 6 小时，报告超过 24 小时未更新说明对账视野已失效。
pub const OPERATIONS_ALERT_RECONCILIATION_STALE_AGE: Duration = Duration::from_secs(24 * 3_600);

/// 年龄水位是否达到固定阈值；负值（时钟回拨归一化）不告警。
pub(super) const fn reached(age_seconds: i64, threshold: Duration) -> bool {
    age_seconds >= 0 && age_seconds >= threshold.as_secs() as i64
}

/// 告警分级闭集（决策 66）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OperationsAlertSeverity {
    /// 需要立即处理；按策略冷却重复通知。
    Urgent,
    /// 非紧急；重复通知并入每日摘要节奏（摘要聚合属于告警台）。
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
            OperationsAlertKind::RecoveryPaymentMailUnsent => format!(
                "付款邮件未寄出但已逾期 {} 封，最长 {} 秒；请先核对付款投递并人工处理",
                self.count, self.age_seconds
            ),
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
            OperationsAlertKind::MailPollStalled => format!(
                "MoogleMail 收件轮询停滞 {} 秒，连续失败 {}；请检查收件凭据与邮箱页",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::MailSentPageLimitApproaching => {
                format!("Sent 列表页数接近上限：{}", self.count)
            }
            OperationsAlertKind::CredentialInvalid => {
                format!("{} 当前凭据失效，请重新录入", self.scope)
            }
            OperationsAlertKind::AuctionPhaseOverdue => format!(
                "场次结束 {} 秒后仍未进入 Final，共 {} 个；请检查 Worker 生命周期",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::ProxyExpiryOverdue => format!(
                "慢速代理到期 {} 秒后仍未执行，共 {} 条；请检查 Worker 与代理任务",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::QueueLatency => format!(
                "投递队列延迟 {} 秒仍未推进，共 {} 条；请检查 Worker 领取",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::ForumSyncFailed => {
                format!("论坛主题发布失败 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::ForumMessageFailed => format!(
                "论坛私信失败 {}，最长 {} 秒；请核对发送账号与私信页",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::ForumTopicPageLimitApproaching => {
                format!("论坛主题远端页数接近上限：{}", self.count)
            }
            OperationsAlertKind::NotificationDeliveryFailed => {
                format!("非结算通知投递失败 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::NotificationPostageInsufficient => {
                format!("非结算通知邮资不足 {}；请补充邮资后重试", self.count)
            }
            OperationsAlertKind::AccountBusy => format!(
                "{} 外部账号已忙 {} 秒，共 {} 次观测",
                self.scope, self.age_seconds, self.count
            ),
            OperationsAlertKind::DiskCapacityLow => format!(
                "备份文件系统可用空间低于配置水位，缺口 {} 字节；请清理旧副本或扩容",
                self.count
            ),
            OperationsAlertKind::BackupFailed => format!(
                "定时备份最近一次运行未成功，已 {} 秒；请核对回执与 journal",
                self.age_seconds
            ),
            OperationsAlertKind::RestoreDrillOverdue if self.count == 0 => {
                "尚无成功的恢复演练记录；请按恢复手册在隔离实例完成一次演练并留存回执".to_owned()
            }
            OperationsAlertKind::RestoreDrillOverdue => format!(
                "最近一次成功的恢复演练已 {} 天；请按恢复手册完成月度演练并留存回执",
                self.age_seconds / 86_400
            ),
            OperationsAlertKind::WalUploadStalled => format!(
                "WAL 归档暂存积压 {} 段，最老 {} 秒未上传；请核对上传器与 PostgreSQL 回收",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::SeasonObligationsPending => {
                format!("换季前未完成 isekai 义务：{}", self.count)
            }
            OperationsAlertKind::CustodyDrift => format!(
                "{} 装备对账差异 {} 件，报告年龄 {} 秒；请核对 Armory 与义务，不自动转移",
                self.scope, self.count, self.age_seconds
            ),
            OperationsAlertKind::SolvencyShortfall => format!(
                "{} 偿付缺口 {} credits，报告年龄 {} 秒；请先核对余额与待付货款，不自动转账",
                self.scope, self.count, self.age_seconds
            ),
            OperationsAlertKind::ReconciliationStale => format!(
                "{} 资产报告已 {} 秒未更新，请检查 Worker 对账",
                self.scope, self.age_seconds
            ),
        };
        let summary = format!("{summary}；处置手册：{}", self.source.runbook_anchor());
        if !auction_domain::valid_text(&summary, 512) {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(summary)
    }
}
