//! 论坛请求指标闭集：操作、边界结果、信号与总时间预算阶段。

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

    /// 是否属于改变远端状态的论坛写入（建主题、回帖、编辑、PM 发送）。
    /// 写入使用请求门的前台档；页面预读、资料读取与 Sent 列表仍属后台只读。
    /// 该分类只决定请求先后，不改变写入未知或自动重发规则。
    pub const fn is_write(self) -> bool {
        matches!(self, Self::CreateTopic | Self::ReplyTopic | Self::EditPost | Self::PmSend)
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

#[cfg(test)]
mod tests {
    use super::ForumOperation;

    #[test]
    fn forum_writes_are_foreground_only_for_state_changing_operations() {
        for operation in [
            ForumOperation::CreateTopic,
            ForumOperation::ReplyTopic,
            ForumOperation::EditPost,
            ForumOperation::PmSend,
        ] {
            assert!(operation.is_write(), "{} 应使用前台档", operation.label());
        }
        for operation in [
            ForumOperation::CheckSession,
            ForumOperation::ReconcileTopic,
            ForumOperation::FetchTopic,
            ForumOperation::ReadPost,
            ForumOperation::Profile,
            ForumOperation::PmPrepare,
            ForumOperation::PmSentList,
            ForumOperation::PmSentDetail,
        ] {
            assert!(!operation.is_write(), "{} 应保持后台档", operation.label());
        }
    }
}
