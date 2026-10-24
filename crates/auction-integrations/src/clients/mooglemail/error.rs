use std::fmt;

use thiserror::Error;

use super::protocol::{MoogleMailOperation, MoogleMailWorld};

/// MoogleMail 适配器允许跨越外部边界的稳定错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailErrorCategory {
    /// 调用参数、固定 endpoint 或本地适配器配置无效。
    Configuration,
    CredentialUnavailable,
    /// 远端返回登录页或登录跳转，当前 Cookie 已失效。
    SessionExpired,
    /// 当前账号处于战斗页，Item Shop 暂时无法读取；Cookie 仍然有效。
    AccountBusy,
    RateLimited,
    /// 网络、超时或远端 5xx 等可重试失败。
    RemoteTemporary,
    /// Sent 扫描达到页数上限仍未到末页；本地预算限制与远端故障分开记录。
    SentScanPageLimit,
    /// Sent 全量扫描耗尽时间预算；发送前只延后，发送后仍保留未知结果。
    SentScanTimeout,
    /// 详情读取恢复队列未能确认持久化，禁止继续读取详情。
    PersistenceUnavailable,
    ResponseTooLarge,
    /// 页面结构、字段或内容类型不再符合冻结协议。
    ProtocolDrift,
    IdentityMismatch,
    /// 认证 HV UID、论坛目标资料或运行时绑定未获联合证明，不能当作“不是 bot”。
    /// 预期名称缺省也必须真实读取；错误不包含远端原文或配置名称。
    IdentityUnproven,
    /// 在完整页窗口中没有观察到要求的 exact remote ID。
    NotObserved,
    /// POST 已经发出但远端结果无法确定，禁止自动重试。
    WriteUnknown,
}

impl MoogleMailErrorCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
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

impl fmt::Display for MoogleMailErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// MoogleMail 外部错误。
/// 错误只保留操作、世界、分类、是否可重试和加密证据引用，不保存 URL、Cookie、token、原始 HTML 或底层
/// HTTP 错误，避免错误被日志或 Admin DTO 间接回显敏感数据。`write_started` 用于区分“读侧
/// 登录失效尚未外写”和“写请求已发出只能标记失效、不能盲目重试”；单 Cookie 不自动轮换。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error(
    "MoogleMail 请求失败: operation={operation} world={world} category={category} temporary={temporary} write_started={write_started} evidence_status={evidence_status} evidence_id={evidence_id:?}"
)]
pub struct MoogleMailError {
    pub operation: MoogleMailOperation,
    pub world: MoogleMailWorld,
    pub category: MoogleMailErrorCategory,
    /// 是否允许由 Worker 在边界内重试。
    pub temporary: bool,
    /// 请求是否已经进入远端写入阶段。
    write_started: bool,
    /// 仅引用独立加密文件；普通错误对象不持有原始响应或密文内容。
    pub evidence_id: Option<uuid::Uuid>,
    /// 留证状态使用固定代码，便于区分缺少响应与保存失败。
    pub evidence_status: &'static str,
    /// 诊断自身失败时的安全分类，不包含文件系统或底层密码库错误文本。
    pub evidence_failure: Option<&'static str>,
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
        Self {
            operation,
            world,
            category,
            temporary,
            write_started: false,
            evidence_id: None,
            evidence_status: "not_captured",
            evidence_failure: None,
        }
    }

    /// 建立已进入远端写入阶段的错误；该结果永远不能被自动重发。
    pub(crate) const fn after_write(
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
        category: MoogleMailErrorCategory,
    ) -> Self {
        Self { write_started: true, ..Self::new(operation, world, category, false) }
    }

    pub const fn is_temporary(self) -> bool {
        self.temporary
    }

    pub const fn write_started(self) -> bool {
        self.write_started
    }

    /// 标识尚未外部写入的会话失效，供调用方撤销当前凭据。
    pub const fn is_session_expired(self) -> bool {
        matches!(self.category, MoogleMailErrorCategory::SessionExpired) && !self.write_started
    }
}
