//! E-Hentai 操作与安全错误分类。

use super::*;

/// 论坛操作名称，用于安全错误分类和日志字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhentaiOperation {
    CheckSession,
    CreateTopic,
    ReconcileTopic,
    FetchTopic,
    ReplyTopic,
    EditPost,
    ReadPostBody,
}

impl EhentaiOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CheckSession => "check_session",
            Self::CreateTopic => "create_topic",
            Self::ReconcileTopic => "reconcile_topic",
            Self::FetchTopic => "fetch_topic",
            Self::ReplyTopic => "reply_topic",
            Self::EditPost => "edit_post",
            Self::ReadPostBody => "read_post_body",
        }
    }
}

/// 论坛适配器对外暴露的稳定错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EhentaiErrorCategory {
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

impl EhentaiErrorCategory {
    /// 返回不包含远端正文和凭据的稳定错误码。
    pub const fn as_str(self) -> &'static str {
        match self {
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

/// E-Hentai 适配器错误。
/// 该错误故意不保存 URL、Cookie、标题、帖子正文或远端响应，避免错误被日志、任务状态或
/// 命令结果意外带出敏感数据。`write_started` 是创建主题的重试栅栏：一旦为真，当前
/// Cookie 不能通过错误自动重试，且任务必须先做远端对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EhentaiError {
    operation: EhentaiOperation,
    category: EhentaiErrorCategory,
    retryable: bool,
    write_started: bool,
}

impl EhentaiError {
    pub const fn operation(self) -> EhentaiOperation {
        self.operation
    }

    pub const fn category(self) -> EhentaiErrorCategory {
        self.category
    }

    /// 返回该错误是否可以由 Worker 退避后再次处理。
    pub const fn retryable(self) -> bool {
        self.retryable
    }

    /// 返回请求是否已经进入远端写入阶段。
    pub const fn write_started(self) -> bool {
        self.write_started
    }

    /// 只有读侧、尚未发生写入时才允许使当前凭据失效；单 Cookie 模型不会自动切换。
    pub const fn is_session_expired(self) -> bool {
        matches!(self.category, EhentaiErrorCategory::SessionExpired) && !self.write_started
    }

    /// 转换为应用层安全错误，不携带远端响应或凭据内容。
    pub const fn into_application_error(self) -> ApplicationError {
        ApplicationError::ForumRemote(ForumRemoteError::new(
            self.category.as_str(),
            self.retryable,
            self.write_started,
        ))
    }

    pub(super) const fn new(
        operation: EhentaiOperation,
        category: EhentaiErrorCategory,
        retryable: bool,
        write_started: bool,
    ) -> Self {
        Self { operation, category, retryable, write_started }
    }

    pub(super) const fn configuration(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::Configuration, false, false)
    }

    pub(super) const fn credential(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::CredentialUnavailable, false, false)
    }

    pub(super) const fn session_expired(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::SessionExpired, false, false)
    }

    pub(super) const fn rate_limited(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::RateLimited, !write_started, write_started)
    }

    pub(super) const fn remote_temporary(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::RemoteTemporary, !write_started, write_started)
    }

    pub(super) const fn cloudflare(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::Cloudflare, true, false)
    }

    pub(super) const fn topic_locked(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::TopicLocked, false, false)
    }

    pub(super) const fn permission_denied(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::PermissionDenied, false, false)
    }

    pub(super) const fn response_too_large(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::ResponseTooLarge, false, false)
    }

    pub(super) const fn protocol(operation: EhentaiOperation, write_started: bool) -> Self {
        Self::new(operation, EhentaiErrorCategory::ProtocolDrift, false, write_started)
    }

    pub(super) const fn write_unknown(operation: EhentaiOperation) -> Self {
        Self::new(operation, EhentaiErrorCategory::WriteUnknown, true, true)
    }
}

impl fmt::Display for EhentaiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "E-Hentai forum operation failed: operation={} category={} retryable={} write_started={}",
            self.operation.as_str(),
            self.category.as_str(),
            self.retryable,
            self.write_started,
        )
    }
}

impl std::error::Error for EhentaiError {}
