//! 数据库观测的固定标签；不接受业务标识、SQL 或错误正文。

/// 数据库事务 owner 初始闭集。
#[derive(Debug, Clone, Copy)]
pub enum DatabaseTransactionOwner {
    Bid,
    Proxy,
    SlowProxy,
    Lifecycle,
    Settlement,
    Fulfillment,
    Funding,
    DeliveryClaim,
    DeliveryFence,
    DeliveryOutcome,
    DeliveryAdmin,
    Recovery,
    MailIntake,
    MailboxSweep,
    Scheduler,
    PublicIdentity,
    PublicWatch,
    Admin,
    RuntimeSwitch,
    EquipmentLoss,
    SentConfirmation,
    DeliveryDisposition,
}

impl DatabaseTransactionOwner {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Proxy => "proxy",
            Self::SlowProxy => "slow_proxy",
            Self::Lifecycle => "lifecycle",
            Self::Settlement => "settlement",
            Self::Fulfillment => "fulfillment",
            Self::Funding => "funding",
            Self::DeliveryClaim => "delivery_claim",
            Self::DeliveryFence => "delivery_fence",
            Self::DeliveryOutcome => "delivery_outcome",
            Self::DeliveryAdmin => "delivery_admin",
            Self::Recovery => "recovery",
            Self::MailIntake => "mail_intake",
            Self::MailboxSweep => "mailbox_sweep",
            Self::Scheduler => "scheduler",
            Self::PublicIdentity => "public_identity",
            Self::PublicWatch => "public_watch",
            Self::Admin => "admin",
            Self::RuntimeSwitch => "runtime_switch",
            Self::EquipmentLoss => "equipment_loss",
            Self::SentConfirmation => "sent_confirmation",
            Self::DeliveryDisposition => "delivery_disposition",
        }
    }

    pub(super) const fn phase(self) -> DatabaseTransactionPhase {
        match self {
            Self::Bid | Self::Proxy | Self::SlowProxy | Self::Lifecycle => {
                DatabaseTransactionPhase::Unknown
            }
            _ => DatabaseTransactionPhase::NotApplicable,
        }
    }
}

/// 只使用事务内已读取的拍卖状态；拿到快照前保留 unknown。
#[derive(Debug, Clone, Copy)]
pub enum DatabaseTransactionPhase {
    Final,
    NonFinal,
    Unknown,
    NotApplicable,
}

impl DatabaseTransactionPhase {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Final => "final",
            Self::NonFinal => "non_final",
            Self::Unknown => "unknown",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// 明确中止的冲突种类；不扩大事务重试白名单。
#[derive(Debug, Clone, Copy)]
pub(super) enum ConflictKind {
    SerializationFailure,
    Deadlock,
}

impl ConflictKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::SerializationFailure => "serialization_failure",
            Self::Deadlock => "deadlock",
        }
    }
}

/// 只对明确报错的序号分配点归因；COMMIT 冲突不能猜测来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseConflictSite {
    RealtimeSequence,
    Other,
    Unknown,
}

impl DatabaseConflictSite {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::RealtimeSequence => "realtime_sequence",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

/// 长期借出的连接单列占用，不混入普通 Worker 循环持有时长。
#[derive(Debug, Clone, Copy)]
pub enum DatabaseReservedPurpose {
    MailOwnership,
    ForumOwnership,
    RealtimeOwnership,
    DeliveryListener,
}

impl DatabaseReservedPurpose {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::MailOwnership => "mail_ownership",
            Self::ForumOwnership => "forum_ownership",
            Self::RealtimeOwnership => "realtime_ownership",
            Self::DeliveryListener => "delivery_listener",
        }
    }
}

/// 单次获取连接的实际结果；取消与超时分开计量。
#[derive(Debug, Clone, Copy)]
pub enum DatabaseAcquireOutcome {
    Success,
    Timeout,
    Error,
    Canceled,
}

impl DatabaseAcquireOutcome {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Timeout => "timeout",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}
