//! W0.4/W0.5 外部客户端指标的稳定闭集与事件。

use std::time::Duration;

/// MoogleMail 世界闭集；邮件绝不使用 shared。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailWorld {
    Main,
    Isekai,
}

impl MailWorld {
    pub const ALL: [Self; 2] = [Self::Main, Self::Isekai];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }
}

/// 论坛世界闭集；shared 只用于没有权威世界上下文的资料读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumWorld {
    Main,
    Isekai,
    Shared,
}

impl ForumWorld {
    pub const ALL: [Self; 3] = [Self::Main, Self::Isekai, Self::Shared];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
            Self::Shared => "shared",
        }
    }
}

/// 邮件物理请求操作；与既有安全错误分类的 operation 字符串保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailOperation {
    Inbox,
    ReadMail,
    Detail,
    Sent,
    SentDetail,
    Balance,
    Equipment,
    Identity,
    Inventory,
    InventoryOrganize,
    EquipmentUnstore,
    EquipmentUnlock,
    Compose,
    AttachAdd,
    AttachCod,
    Send,
    ReturnMessage,
    AttachRemove,
    Discard,
}

impl MailOperation {
    pub const ALL: [Self; 19] = [
        Self::Inbox,
        Self::ReadMail,
        Self::Detail,
        Self::Sent,
        Self::SentDetail,
        Self::Balance,
        Self::Equipment,
        Self::Identity,
        Self::Inventory,
        Self::InventoryOrganize,
        Self::EquipmentUnstore,
        Self::EquipmentUnlock,
        Self::Compose,
        Self::AttachAdd,
        Self::AttachCod,
        Self::Send,
        Self::ReturnMessage,
        Self::AttachRemove,
        Self::Discard,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::ReadMail => "read_mail",
            Self::Detail => "detail",
            Self::Sent => "sent",
            Self::SentDetail => "sent_detail",
            Self::Balance => "balance",
            Self::Equipment => "equipment",
            Self::Identity => "identity",
            Self::Inventory => "inventory",
            Self::InventoryOrganize => "inventory_organize",
            Self::EquipmentUnstore => "equipment_unstore",
            Self::EquipmentUnlock => "equipment_unlock",
            Self::Compose => "compose",
            Self::AttachAdd => "attach_add",
            Self::AttachCod => "attach_cod",
            Self::Send => "send",
            Self::ReturnMessage => "return_message",
            Self::AttachRemove => "attach_remove",
            Self::Discard => "discard",
        }
    }
}

/// 论坛逻辑请求操作；PM、资料读取与普通主题读取分别归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumOperation {
    CheckSession,
    CreateTopic,
    ReconcileTopic,
    FetchTopic,
    ReplyTopic,
    EditPost,
    ReadPost,
    Profile,
    PmPrepare,
    PmSend,
    PmSentList,
    PmSentDetail,
}

impl ForumOperation {
    pub const ALL: [Self; 12] = [
        Self::CheckSession,
        Self::CreateTopic,
        Self::ReconcileTopic,
        Self::FetchTopic,
        Self::ReplyTopic,
        Self::EditPost,
        Self::ReadPost,
        Self::Profile,
        Self::PmPrepare,
        Self::PmSend,
        Self::PmSentList,
        Self::PmSentDetail,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CheckSession => "check_session",
            Self::CreateTopic => "create_topic",
            Self::ReconcileTopic => "reconcile_topic",
            Self::FetchTopic => "fetch_topic",
            Self::ReplyTopic => "reply_topic",
            Self::EditPost => "edit_post",
            Self::ReadPost => "read_post",
            Self::Profile => "profile",
            Self::PmPrepare => "pm_prepare",
            Self::PmSend => "pm_send",
            Self::PmSentList => "pm_sent_list",
            Self::PmSentDetail => "pm_sent_detail",
        }
    }
}

/// 邮件请求边界结果；ok 只表示请求终结，不表示资产或投递证明完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailRequestCategory {
    Ok,
    Canceled,
    Configuration,
    CredentialUnavailable,
    SessionExpired,
    AccountBusy,
    RateLimited,
    RemoteTemporary,
    SentScanPageLimit,
    SentScanTimeout,
    PersistenceUnavailable,
    ResponseTooLarge,
    ProtocolDrift,
    IdentityMismatch,
    IdentityUnproven,
    NotObserved,
    WriteUnknown,
}

impl MailRequestCategory {
    pub const ALL: [Self; 17] = [
        Self::Ok,
        Self::Canceled,
        Self::Configuration,
        Self::CredentialUnavailable,
        Self::SessionExpired,
        Self::AccountBusy,
        Self::RateLimited,
        Self::RemoteTemporary,
        Self::SentScanPageLimit,
        Self::SentScanTimeout,
        Self::PersistenceUnavailable,
        Self::ResponseTooLarge,
        Self::ProtocolDrift,
        Self::IdentityMismatch,
        Self::IdentityUnproven,
        Self::NotObserved,
        Self::WriteUnknown,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Canceled => "canceled",
            Self::Configuration => "configuration",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::SessionExpired => "session_expired",
            Self::AccountBusy => "account_busy",
            Self::RateLimited => "rate_limited",
            Self::RemoteTemporary => "remote_temporary",
            Self::SentScanPageLimit => "sent_scan_page_limit",
            Self::SentScanTimeout => "sent_scan_timeout",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::ResponseTooLarge => "response_too_large",
            Self::ProtocolDrift => "protocol_drift",
            Self::IdentityMismatch => "identity_mismatch",
            Self::IdentityUnproven => "identity_unproven",
            Self::NotObserved => "not_observed",
            Self::WriteUnknown => "write_unknown",
        }
    }
}

/// 论坛请求边界结果；ok 不表示写后证明或 PM 已送达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumRequestCategory {
    Ok,
    Canceled,
    Configuration,
    CredentialUnavailable,
    SessionExpired,
    RateLimited,
    RemoteTemporary,
    Cloudflare,
    TopicLocked,
    PermissionDenied,
    ResponseTooLarge,
    ProtocolDrift,
    WriteUnknown,
}

impl ForumRequestCategory {
    pub const ALL: [Self; 13] = [
        Self::Ok,
        Self::Canceled,
        Self::Configuration,
        Self::CredentialUnavailable,
        Self::SessionExpired,
        Self::RateLimited,
        Self::RemoteTemporary,
        Self::Cloudflare,
        Self::TopicLocked,
        Self::PermissionDenied,
        Self::ResponseTooLarge,
        Self::ProtocolDrift,
        Self::WriteUnknown,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Canceled => "canceled",
            Self::Configuration => "configuration",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::SessionExpired => "session_expired",
            Self::RateLimited => "rate_limited",
            Self::RemoteTemporary => "remote_temporary",
            Self::Cloudflare => "cloudflare",
            Self::TopicLocked => "topic_locked",
            Self::PermissionDenied => "permission_denied",
            Self::ResponseTooLarge => "response_too_large",
            Self::ProtocolDrift => "protocol_drift",
            Self::WriteUnknown => "write_unknown",
        }
    }
}

/// 邮件请求中识别到的额外信号；可与 write_unknown 结果同时出现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailSignal {
    AccountBusy,
    RateLimited,
    Timeout,
}

impl MailSignal {
    pub const fn label(self) -> &'static str {
        match self {
            Self::AccountBusy => "account_busy",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
        }
    }
}

/// 论坛请求信号；错误结果仍保留原分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumSignal {
    RateLimited,
    Timeout,
}

impl ForumSignal {
    pub const fn label(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
        }
    }
}

/// 请求门等待结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Acquired,
    Error,
    Canceled,
}

impl WaitOutcome {
    pub const ALL: [Self; 3] = [Self::Acquired, Self::Error, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

/// 邮件长期操作锁等待结果；锁只记录是否获准。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLockOutcome {
    Acquired,
    Canceled,
}

impl MailLockOutcome {
    pub const ALL: [Self; 2] = [Self::Acquired, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Acquired => "acquired",
            Self::Canceled => "canceled",
        }
    }
}

/// 邮件锁种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLock {
    Exclusive,
    IncomingRead,
    IncomingWrite,
}

impl MailLock {
    pub const ALL: [Self; 3] = [Self::Exclusive, Self::IncomingRead, Self::IncomingWrite];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Exclusive => "exclusive",
            Self::IncomingRead => "incoming_read",
            Self::IncomingWrite => "incoming_write",
        }
    }
}

/// 邮件预算 owner 阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailPhase {
    PollFetch,
    PollBatch,
    Maintenance,
    ReadObservation,
    SharedRead,
    SentScan,
    ReadScan,
    SentBaselineBefore,
    SentBaselineAfter,
}

impl MailPhase {
    pub const ALL: [Self; 9] = [
        Self::PollFetch,
        Self::PollBatch,
        Self::Maintenance,
        Self::ReadObservation,
        Self::SharedRead,
        Self::SentScan,
        Self::ReadScan,
        Self::SentBaselineBefore,
        Self::SentBaselineAfter,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::PollFetch => "poll_fetch",
            Self::PollBatch => "poll_batch",
            Self::Maintenance => "maintenance",
            Self::ReadObservation => "read_observation",
            Self::SharedRead => "shared_read",
            Self::SentScan => "sent_scan",
            Self::ReadScan => "read_scan",
            Self::SentBaselineBefore => "sent_baseline_before",
            Self::SentBaselineAfter => "sent_baseline_after",
        }
    }
}

/// 邮件预算类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailBudget {
    Time,
    Pages,
}

impl MailBudget {
    pub const ALL: [Self; 2] = [Self::Time, Self::Pages];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Pages => "pages",
        }
    }
}

/// 主题扫描阶段；本批只记录全量扫描。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanMode {
    Full,
}

impl ForumTopicScanMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
        }
    }
}

/// 主题扫描页数口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanMeasure {
    Discovered,
    Processed,
}

impl ForumTopicScanMeasure {
    pub const ALL: [Self; 2] = [Self::Discovered, Self::Processed];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Processed => "processed",
        }
    }
}

/// 主题扫描终结原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanOutcome {
    Complete,
    PageLimit,
    Error,
    Canceled,
}

impl ForumTopicScanOutcome {
    pub const ALL: [Self; 4] = [Self::Complete, Self::PageLimit, Self::Error, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::PageLimit => "page_limit",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

/// PM 逻辑阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumPmPhase {
    Prepare,
    Send,
    Reconcile,
}

impl ForumPmPhase {
    pub const ALL: [Self; 3] = [Self::Prepare, Self::Send, Self::Reconcile];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Send => "send",
            Self::Reconcile => "reconcile",
        }
    }
}

/// PM 逻辑结果；not_observed 不表示未发送。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumPmOutcome {
    Prepared,
    Proved,
    Unknown,
    NotObserved,
    Error,
    Canceled,
}

impl ForumPmOutcome {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Proved => "proved",
            Self::Unknown => "unknown",
            Self::NotObserved => "not_observed",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

/// PM 安全分类；none 只用于已证明成功或尚未进入外部边界的取消。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumPmCategory {
    None,
    Unavailable,
    Authentication,
    RateLimited,
    InvalidRecipient,
    Protocol,
    OutcomeUnknown,
}

impl ForumPmCategory {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Unavailable,
        Self::Authentication,
        Self::RateLimited,
        Self::InvalidRecipient,
        Self::Protocol,
        Self::OutcomeUnknown,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unavailable => "unavailable",
            Self::Authentication => "authentication",
            Self::RateLimited => "rate_limited",
            Self::InvalidRecipient => "invalid_recipient",
            Self::Protocol => "protocol",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

/// 论坛总时间预算 owner 阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumBudgetPhase {
    TopicItem,
    PmItem,
    Profile,
}

impl ForumBudgetPhase {
    pub const ALL: [Self; 3] = [Self::TopicItem, Self::PmItem, Self::Profile];

    pub const fn label(self) -> &'static str {
        match self {
            Self::TopicItem => "topic_item",
            Self::PmItem => "pm_item",
            Self::Profile => "profile",
        }
    }
}

/// 导入时观察到的拍卖阶段；未知不与 normal/final 合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayPhase {
    Normal,
    Final,
    Unknown,
}

impl ReplyDelayPhase {
    pub const ALL: [Self; 3] = [Self::Normal, Self::Final, Self::Unknown];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Final => "final",
            Self::Unknown => "unknown",
        }
    }
}

/// 回帖延迟估计方法；posted_minute 留待后续核实后扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayMethod {
    PollWindow,
}

impl ReplyDelayMethod {
    pub const fn label(self) -> &'static str {
        match self {
            Self::PollWindow => "poll_window",
        }
    }
}

/// 延迟估计上下界字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayBound {
    Lower,
    Midpoint,
    Upper,
}

impl ReplyDelayBound {
    pub const ALL: [Self; 3] = [Self::Lower, Self::Midpoint, Self::Upper];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Midpoint => "midpoint",
            Self::Upper => "upper",
        }
    }
}

/// 延迟采样覆盖结果；缺样不是零延迟。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelaySampleOutcome {
    Observed,
    MissingBaseline,
    CoverageUnknown,
    StateEvicted,
    TimestampUnavailable,
    ClockInvalid,
}

impl ReplyDelaySampleOutcome {
    pub const ALL: [Self; 6] = [
        Self::Observed,
        Self::MissingBaseline,
        Self::CoverageUnknown,
        Self::StateEvicted,
        Self::TimestampUnavailable,
        Self::ClockInvalid,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::MissingBaseline => "missing_baseline",
            Self::CoverageUnknown => "coverage_unknown",
            Self::StateEvicted => "state_evicted",
            Self::TimestampUnavailable => "timestamp_unavailable",
            Self::ClockInvalid => "clock_invalid",
        }
    }
}

/// 同步观测事件；数值只来自调用点已有的时钟或已观察页数。
#[derive(Debug, Clone, Copy)]
pub enum ExternalMetricsEvent {
    MailRequestStarted {
        world: MailWorld,
        operation: MailOperation,
    },
    MailRequestFinished {
        world: MailWorld,
        operation: MailOperation,
        category: MailRequestCategory,
    },
    MailRequestSignal {
        world: MailWorld,
        operation: MailOperation,
        signal: MailSignal,
    },
    MailGateWait {
        world: MailWorld,
        operation: MailOperation,
        outcome: WaitOutcome,
        waited: Duration,
    },
    MailLockWait {
        world: MailWorld,
        operation: MailOperation,
        lock: MailLock,
        outcome: MailLockOutcome,
        waited: Duration,
    },
    MailBudgetExhausted {
        world: MailWorld,
        phase: MailPhase,
        budget: MailBudget,
    },
    MailRequestInterval {
        interval: Duration,
    },
    ForumRequestStarted {
        world: ForumWorld,
        operation: ForumOperation,
    },
    ForumRequestFinished {
        world: ForumWorld,
        operation: ForumOperation,
        category: ForumRequestCategory,
    },
    ForumRequestSignal {
        world: ForumWorld,
        operation: ForumOperation,
        signal: ForumSignal,
    },
    ForumGateWait {
        world: ForumWorld,
        operation: ForumOperation,
        outcome: WaitOutcome,
        waited: Duration,
    },
    ForumRequestInterval {
        interval: Duration,
    },
    ForumTopicScanPages {
        world: ForumWorld,
        mode: ForumTopicScanMode,
        measure: ForumTopicScanMeasure,
        outcome: ForumTopicScanOutcome,
        pages: usize,
    },
    ForumWriteUnknown {
        world: ForumWorld,
        operation: ForumOperation,
    },
    ForumPmOperation {
        world: ForumWorld,
        phase: ForumPmPhase,
        outcome: ForumPmOutcome,
        category: ForumPmCategory,
    },
    ForumBudgetExhausted {
        world: ForumWorld,
        phase: ForumBudgetPhase,
    },
    ForumReplyDelayEstimate {
        world: ForumWorld,
        phase: ReplyDelayPhase,
        method: ReplyDelayMethod,
        lower: Duration,
        midpoint: Duration,
        upper: Duration,
    },
    ForumReplyDelaySample {
        world: ForumWorld,
        phase: ReplyDelayPhase,
        method: ReplyDelayMethod,
        outcome: ReplyDelaySampleOutcome,
    },
}
