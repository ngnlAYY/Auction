//! 持久化告警种类闭集、稳定名称，以及每个种类的分级与重复通知间隔。

use std::fmt;
use std::time::Duration;

use serde::Serialize;

use super::severity::{OPERATIONS_ALERT_DIGEST_INTERVAL, OperationsAlertSeverity};

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
    /// 已寄出、只等收件人领取的履约有寄出时长达到配置阈值的（计数为超期件数）；只提醒人工跟进。
    FulfillmentCollectionOverdue,
    /// 已绑定原派发、尚未处理完的结算退件案件（待决定、领回中、待补寄）；按未结案案件计数，
    /// 不要求存在义务。告警确认或静音不改变案件，案件结案后由扫描关闭。
    SettlementReturnPending,
    /// 来源、绑定或领回证明存在问题的结算退件案件（untrusted/unbound 隔离、待重新核验、Take
    /// 结果未知）；只能经核对、只读核验或系统可验证证明收口。
    SettlementReturnQuarantined,
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
    /// 超过固定时限没有成功的恢复演练（恢复回执或通过的迁移演练），或从未记录过。
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
    /// 持久化种类闭集；与迁移 005/020/085/091/095/099/101/104/110 的
    /// `operations_alert_signals.kind` CHECK 一致。
    pub const ALL: [Self; 40] = [
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
        Self::FulfillmentCollectionOverdue,
        Self::SettlementReturnPending,
        Self::SettlementReturnQuarantined,
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
            Self::FulfillmentCollectionOverdue => "fulfillment_collection_overdue",
            Self::SettlementReturnPending => "settlement_return_pending",
            Self::SettlementReturnQuarantined => "settlement_return_quarantined",
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

    /// 告警分级：付款逾期、待重列等自动恢复的等待类信号属于每日摘要；
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
            // 长期未领取是等收件人操作的等待类信号，持续数天；资产在邮件里、系统无故障，
            // 按每日摘要提醒人工跟进，不按紧急冷却反复通知。
            | Self::FulfillmentCollectionOverdue
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
