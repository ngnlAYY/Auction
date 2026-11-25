//! 受限权利存储端口：找回、挑战、核验、会话读取与续办。
//!
//! 真实、虚拟与未匹配目标必须走同一段写路径并回同形结果；存储适配器负责事务、
//! 计数行唯一约束与持久化，业务规则留在 `auction-domain` 的 `rights`。

use std::fmt;
use std::net::IpAddr;

use auction_domain::consignment::rights::{
    ChallengeCode, LocatorDigest, RightAction, RightSessionToken, RightView, RightsError,
    SessionDigest,
};
use auction_domain::{CommandId, SettlementWorld};
use auction_time::Timestamp;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::RightCryptoError;
use crate::ports::core::{RequestHash, TransactionFuture};

/// 权利端口的装箱异步结果。
pub type RightsFuture<'a, T> = TransactionFuture<'a, T, RightsStoreError>;

/// 权利用例对外的安全错误，不保留依赖错误链或输入原文。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RightsStoreError {
    /// 输入形状无效（定位符、身份、命令 ID）。
    #[error("权利请求无效")]
    InvalidRequest,
    /// 虚拟、错误、过期、已消费或失败用尽；各原因同形。
    #[error("挑战无效")]
    ChallengeInvalid,
    /// 会话或权利已过期。
    #[error("权利已过期")]
    RightExpired,
    /// 动作不在服务端核准的范围。
    #[error("动作不被允许")]
    ActionNotAllowed,
    /// 权利版本已变化。
    #[error("权利版本已变化")]
    StaleVersion,
    /// 浏览器或 IP 预算用尽；与目标是否真实无关。
    #[error("请求过于频繁")]
    BudgetExceeded,
    /// 缺少、格式错误或已失效的会话；统一按未认证处理。
    #[error("会话未认证")]
    Unauthenticated,
    /// 同一命令 ID 对应不同请求。
    #[error("命令编号已用于不同请求")]
    CommandIdConflict,
    /// 存储、密码能力缺失或事务重试耗尽；失败关闭。
    #[error("权利服务暂不可用")]
    Unavailable,
}

impl RightsStoreError {
    /// API 可见的状态码与固定错误码。
    pub const fn api_error(self) -> (u16, &'static str) {
        match self {
            Self::InvalidRequest => (400, "invalid_request"),
            Self::ChallengeInvalid => (403, "challenge_invalid"),
            Self::RightExpired => (409, "right_expired"),
            Self::ActionNotAllowed => (403, "action_not_allowed"),
            Self::StaleVersion => (409, "stale_version"),
            Self::BudgetExceeded => (429, "rate_limited"),
            Self::Unauthenticated => (401, "unauthenticated"),
            Self::CommandIdConflict => (409, "command_id_conflict"),
            Self::Unavailable => (503, "rights_unavailable"),
        }
    }
}

impl From<RightsError> for RightsStoreError {
    /// 领域拒绝到对外错误：格式类归为无效请求；挑战类保持同形。
    fn from(error: RightsError) -> Self {
        match error {
            RightsError::ChallengeInvalid => Self::ChallengeInvalid,
            RightsError::SessionExpired => Self::RightExpired,
            RightsError::BudgetExceeded => Self::BudgetExceeded,
            RightsError::Locator(_)
            | RightsError::SessionToken(_)
            | RightsError::CodeLength
            | RightsError::CodeAlphabet
            | RightsError::CodeNonCanonical => Self::InvalidRequest,
        }
    }
}

impl From<RightCryptoError> for RightsStoreError {
    /// 密码能力失败一律失败关闭，不回显底层原因。
    fn from(_: RightCryptoError) -> Self {
        Self::Unavailable
    }
}

/// `hv_right_browser` cookie 随机量的 SHA-256 摘要；`Debug` 打码。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BrowserBinding(pub [u8; 32]);

impl fmt::Debug for BrowserBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// 已校验的游戏角色名（冻结身份的输入形式）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GameIdentity(String);

impl GameIdentity {
    /// 角色名长度上限（字符）。
    pub const MAX_CHARS: usize = 64;

    /// 严格校验：非空、无首尾空白、无控制字符、不超长；不静默修正。
    pub fn parse(text: &str) -> Option<Self> {
        let valid = !text.is_empty()
            && text == text.trim()
            && text.chars().count() <= Self::MAX_CHARS
            && !text.chars().any(char::is_control);
        valid.then(|| Self(text.to_owned()))
    }

    /// 借用文本。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 找回命令：按游戏身份与世界，向冻结身份投递定位符指引。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocateRightCommand {
    /// 浏览器 UUIDv4。
    pub command_id: CommandId,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 可信客户端 IP。
    pub client_ip: IpAddr,
    /// 游戏身份。
    pub game_identity: GameIdentity,
    /// 世界。
    pub world: SettlementWorld,
    /// 规范摘要，绑定浏览器与全部输入。
    pub request_hash: RequestHash,
}

/// 挑战命令：按定位符摘要发码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeRightCommand {
    /// 浏览器 UUIDv4。
    pub command_id: CommandId,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 可信客户端 IP。
    pub client_ip: IpAddr,
    /// 定位符摘要。
    pub locator_digest: LocatorDigest,
    /// 规范摘要。
    pub request_hash: RequestHash,
}

/// 核验命令。
#[derive(Debug, Clone)]
pub struct VerifyRightCommand {
    /// 浏览器 UUIDv4。
    pub command_id: CommandId,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 挑战 UUID。
    pub challenge_id: Uuid,
    /// 挑战码；`Debug` 打码。
    pub code: ChallengeCode,
    /// 规范摘要，不含码明文。
    pub request_hash: RequestHash,
}

/// 续办命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRightCommand {
    /// 受限人 UUIDv4。
    pub command_id: CommandId,
    /// 会话对应的权利；摘要绑定它，不绑定会话令牌。
    pub right_id: Uuid,
    /// 会话令牌摘要。
    pub session: SessionDigest,
    /// 浏览器绑定。
    pub browser: BrowserBinding,
    /// 已读到的权利版本。
    pub expected_version: u64,
    /// 请求动作。
    pub action: RightAction,
    /// 规范摘要，绑定 `right_id`、版本与动作，不含会话令牌。
    pub request_hash: RequestHash,
}

/// 找回与挑战的同形受理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChallengeIssued {
    /// 挑战 UUID（虚拟目标也有，形状相同）。
    pub challenge_id: Uuid,
    /// 截止。
    pub expires_at: Timestamp,
}

/// 核验成功：新授予的会话。令牌只给 cookie，`Debug` 打码。
#[derive(Debug, Clone)]
pub struct RightVerified {
    /// 权利 UUID。
    pub right_id: Uuid,
    /// 权利版本。
    pub right_version: u64,
    /// 会话截止（原截止，轮换不延寿）。
    pub expires_at: Timestamp,
    /// 会话令牌。
    pub session_token: RightSessionToken,
}

/// 续办处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimDisposition {
    /// 本命令受理。
    ClaimAccepted,
    /// 另一命令已在履约。
    AlreadyFulfilling,
    /// 已完成。
    AlreadyCompleted,
}

impl ClaimDisposition {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaimAccepted => "claim_accepted",
            Self::AlreadyFulfilling => "already_fulfilling",
            Self::AlreadyCompleted => "already_completed",
        }
    }

    /// 首次返回的 HTTP 状态。
    pub const fn status(self) -> u16 {
        match self {
            Self::ClaimAccepted => 202,
            Self::AlreadyFulfilling | Self::AlreadyCompleted => 200,
        }
    }
}

/// 续办结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimOutcome {
    /// 处置。
    pub disposition: ClaimDisposition,
    /// 权利 UUID。
    pub right_id: Uuid,
    /// 权利版本。
    pub right_version: u64,
}

/// 续办命令回执；原受益人重新验权后可查旧回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RightReceipt {
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 权利 UUID。
    pub right_id: Uuid,
    /// 处置。
    pub disposition: ClaimDisposition,
}

/// 受限权利存储端口。
pub trait RightsStore: Send + Sync {
    /// 找回：真实、虚拟、未匹配目标回同形结果；只向冻结身份写定位符指引 outbox。
    fn locate(&self, command: LocateRightCommand) -> RightsFuture<'_, ChallengeIssued>;

    /// 发码：同形结果；每权利每 10 分钟至多一封。
    fn challenge(&self, command: ChallengeRightCommand) -> RightsFuture<'_, ChallengeIssued>;

    /// 核验：虚拟、错误、过期、超次统一 `ChallengeInvalid`；成功发单权利会话，同命令重放轮换令牌。
    fn verify(&self, command: VerifyRightCommand) -> RightsFuture<'_, RightVerified>;

    /// 读取会话对应的权利视图；浏览器不符、过期或未知为 `None`。
    fn current(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
    ) -> RightsFuture<'_, Option<RightView>>;

    /// 续办：在锁内核对版本与动作范围，同命令同请求返回原结果。
    fn claim(&self, command: ClaimRightCommand) -> RightsFuture<'_, ClaimOutcome>;

    /// 查本权利下的命令回执。
    fn command_receipt(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
        command_id: CommandId,
    ) -> RightsFuture<'_, Option<RightReceipt>>;
}
