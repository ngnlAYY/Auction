//! 论坛 PM 指标闭集：逻辑阶段、结果与安全分类。

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
