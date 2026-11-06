//! MoogleMail 请求指标闭集：操作、边界结果、信号、预算与说明信类别。

/// MoogleMail 说明信预算的稳定类别标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailNoticeClass {
    /// 注册或登录 key 轮换的账号回信。
    AccountReply,
    /// 拒收说明、换绑结果与论坛回执等发件人说明信。
    Explanation,
}

impl MailNoticeClass {
    pub const ALL: [Self; 2] = [Self::AccountReply, Self::Explanation];

    pub const fn label(self) -> &'static str {
        match self {
            Self::AccountReply => "account_reply",
            Self::Explanation => "explanation",
        }
    }
}

/// MoogleMail 物理请求操作；与既有安全错误分类的 operation 字符串保持一致。
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
