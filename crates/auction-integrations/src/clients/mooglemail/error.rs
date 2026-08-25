use std::fmt;

use thiserror::Error;

use super::protocol::{MoogleMailOperation, MoogleMailWorld};

/// MoogleMail 适配器允许跨越外部边界的稳定错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailErrorCategory {
    /// 调用参数、固定 endpoint 或本地适配器配置无效。
    Configuration,
    /// 当前没有可以安全使用的 Cookie。
    CredentialUnavailable,
    /// 远端返回登录页或登录跳转，当前 Cookie 已失效。
    SessionExpired,
    /// 远端明确要求降低请求频率。
    RateLimited,
    /// 网络、超时或远端 5xx 等可重试失败。
    RemoteTemporary,
    /// 响应超过适配器的硬边界。
    ResponseTooLarge,
    /// 页面结构、字段或内容类型不再符合冻结协议。
    ProtocolDrift,
    /// 摘要、详情或装备身份之间不一致。
    IdentityMismatch,
    /// 在完整页窗口中没有观察到要求的 exact remote ID。
    NotObserved,
    /// POST 已经发出但远端结果无法确定，禁止自动重试。
    WriteUnknown,
}

impl MoogleMailErrorCategory {
    /// 返回持久化和日志中使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::SessionExpired => "session_expired",
            Self::RateLimited => "rate_limited",
            Self::RemoteTemporary => "remote_temporary",
            Self::ResponseTooLarge => "response_too_large",
            Self::ProtocolDrift => "protocol_drift",
            Self::IdentityMismatch => "identity_mismatch",
            Self::NotObserved => "not_observed",
            Self::WriteUnknown => "write_unknown",
        }
    }
}

impl fmt::Display for MoogleMailErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// MoogleMail 外部错误。
///
/// 错误只保留操作、世界、分类和是否可重试，不保存 URL、Cookie、token、原始 HTML 或底层
/// HTTP 错误，避免错误被日志或 Admin DTO 间接回显敏感数据。`write_started` 用于区分“读侧
/// 登录失效可安全轮换”和“写请求已发出只能标记失效、不能盲目重试”。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error(
    "MoogleMail 请求失败: operation={operation} world={world} category={category} temporary={temporary} write_started={write_started}"
)]
pub struct MoogleMailError {
    /// 发生错误的远端操作。
    pub operation: MoogleMailOperation,
    /// 发生错误的游戏世界。
    pub world: MoogleMailWorld,
    /// 稳定错误分类。
    pub category: MoogleMailErrorCategory,
    /// 是否允许由 Worker 在边界内重试。
    pub temporary: bool,
    /// 请求是否已经进入远端写入阶段。
    write_started: bool,
}

impl MoogleMailError {
    /// 建立协议结构不一致的读侧错误，供 Worker 边界拒绝无法表示的投影值。
    pub const fn protocol_drift(operation: MoogleMailOperation, world: MoogleMailWorld) -> Self {
        Self::new(operation, world, MoogleMailErrorCategory::ProtocolDrift, false)
    }

    pub(crate) const fn new(
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
        category: MoogleMailErrorCategory,
        temporary: bool,
    ) -> Self {
        Self { operation, world, category, temporary, write_started: false }
    }

    /// 建立已进入远端写入阶段的错误；该结果永远不能被自动重发。
    pub(crate) const fn after_write(
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
        category: MoogleMailErrorCategory,
    ) -> Self {
        Self { operation, world, category, temporary: false, write_started: true }
    }

    /// 返回是否可以由外层有限退避重试。
    pub const fn is_temporary(self) -> bool {
        self.temporary
    }

    /// 返回请求是否已经开始远端写入。
    pub const fn write_started(self) -> bool {
        self.write_started
    }

    /// 只有读侧的 session 失效才允许尝试下一个凭据。
    pub const fn is_session_expired(self) -> bool {
        matches!(self.category, MoogleMailErrorCategory::SessionExpired) && !self.write_started
    }
}
