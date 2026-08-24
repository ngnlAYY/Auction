//! Admin Users owner 的用户管理端口。
//!
//! 该模块覆盖旧版 A28/A30/A31 的稳定业务边界：用户列表、创建、身份修改、登录 key
//! 撤销/重签、黑名单、改名回滚以及命令幂等。明文登录 key 只存在于短生命周期值对象中，
//! storage 只接收摘要和密文信封；响应重放需要保留命令当时的密文信封，不能读取当前 key
//! 冒充历史命令结果。

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::SettlementId;

use super::user_mail::{MailLoginKey, MailLoginKeyEnvelope};
use super::{ApplicationFuture, RequestHash};

/// Admin 用户列表允许的最大页大小。
pub const MAX_ADMIN_USER_PAGE_SIZE: i64 = 200;
/// Admin 用户列表允许的最大偏移。
pub const MAX_ADMIN_USER_OFFSET: i64 = 2_147_483_647;
/// recovery 因 COD 超时触发买家黑名单时使用的固定公开原因。
pub const RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON: &str = "拍卖 COD 超时未收取";
/// recovery 代 Users owner 写入黑名单时使用的固定 actor。
pub const RECOVERY_BUYER_BLACKLIST_ACTOR: &str = "auction-recovery";
/// 写入 settlement recovery action 的黑名单完成证明类型。
pub const RECOVERY_BUYER_BLACKLIST_PROOF_KIND: &str = "users_recovery_buyer_blacklisted";

/// Admin 用户状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminUserStatus {
    Active,
    Blacklisted,
}

impl AdminUserStatus {
    /// 返回数据库与 HTTP 契约使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blacklisted => "blacklisted",
        }
    }

    /// 从数据库字符串解析闭集状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "blacklisted" => Some(Self::Blacklisted),
            _ => None,
        }
    }
}

/// Admin 登录 key 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminUserKeyStatus {
    Active,
    Revoked,
}

impl AdminUserKeyStatus {
    /// 返回数据库和 HTTP 契约使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    /// 从数据库字符串解析闭集状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

/// 不含登录 key、摘要或密文的用户管理安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUser {
    pub id: Uuid,
    pub username: String,
    pub game_player_id: Option<String>,
    pub status: AdminUserStatus,
    pub key_status: AdminUserKeyStatus,
    pub state_version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AdminUser {
    /// 校验 owner 返回的用户投影。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && valid_text(&self.username, 256)
            && valid_optional_text(self.game_player_id.as_deref(), 256)
            && self.state_version > 0
            && valid_time(self.created_at)
            && valid_time(self.updated_at)
            && self.updated_at >= self.created_at
    }
}

/// Admin 用户列表中展示的黑名单历史安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUserBlacklistHistoryEntry {
    pub public_reason: String,
    pub blacklisted_at: DateTime<Utc>,
}

impl AdminUserBlacklistHistoryEntry {
    /// 校验历史记录不携带内部 actor 或敏感材料，且时间与原因可安全展示。
    pub fn validate(&self) -> bool {
        valid_text(&self.public_reason, 1024) && valid_time(self.blacklisted_at)
    }
}

/// Admin 用户列表条目；黑名单历史只在列表查询返回，不进入写命令响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminUserListItem {
    pub user: AdminUser,
    pub blacklist_count: i64,
    pub blacklist_history: Vec<AdminUserBlacklistHistoryEntry>,
}

impl AdminUserListItem {
    /// 校验历史最多返回最近十条，并保持数据库的倒序排列。
    pub fn validate(&self) -> bool {
        self.user.validate()
            && self.blacklist_count >= 0
            && self.blacklist_history.len()
                == self.blacklist_count.min(10).try_into().unwrap_or(usize::MAX)
            && self.blacklist_history.iter().all(|entry| entry.validate())
            && self
                .blacklist_history
                .windows(2)
                .all(|entries| entries[0].blacklisted_at >= entries[1].blacklisted_at)
    }
}

/// recovery 请求 Users owner 确保买家进入黑名单的幂等命令。
///
/// `command_id` 和 `request_hash` 由 Worker 按 recovery action 稳定派生；Users owner 负责在
/// 自己的命令账本中重放结果，避免 Worker 重启或外部重试重复写入黑名单历史。
#[derive(Debug, Clone)]
pub struct RecoveryBuyerBlacklistCommand {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub recovery_id: SettlementId,
    pub action_id: SettlementId,
    pub buyer_user_id: Uuid,
    pub public_reason: String,
    pub actor_id: String,
    pub occurred_at: DateTime<Utc>,
}

impl RecoveryBuyerBlacklistCommand {
    /// 校验 recovery 固定命令字段和不含敏感数据的请求摘要。
    pub fn validate(&self) -> bool {
        !self.command_id.is_nil()
            && !self.request_hash.as_bytes().iter().all(|byte| *byte == 0)
            && !self.recovery_id.is_nil()
            && !self.action_id.is_nil()
            && !self.buyer_user_id.is_nil()
            && self.public_reason == RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON
            && self.actor_id == RECOVERY_BUYER_BLACKLIST_ACTOR
            && valid_time(self.occurred_at)
    }
}

/// Users owner 完成 recovery 买家黑名单命令后返回的安全结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryBuyerBlacklistResult {
    pub command_id: Uuid,
    pub recovery_id: SettlementId,
    pub action_id: SettlementId,
    pub buyer_user_id: Uuid,
    pub user: AdminUser,
    pub public_reason: String,
    pub actor_id: String,
    pub occurred_at: DateTime<Utc>,
}

impl RecoveryBuyerBlacklistResult {
    /// 校验结果仍然绑定原 recovery/action，且不会把 key 材料带回 Worker。
    pub fn validate(&self) -> bool {
        !self.command_id.is_nil()
            && !self.recovery_id.is_nil()
            && !self.action_id.is_nil()
            && !self.buyer_user_id.is_nil()
            && self.user.id == self.buyer_user_id
            && self.user.status == AdminUserStatus::Blacklisted
            && self.public_reason == RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON
            && self.actor_id == RECOVERY_BUYER_BLACKLIST_ACTOR
            && valid_time(self.occurred_at)
            && self.user.validate()
    }
}

/// Admin 用户列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUserListRequest {
    pub name: Option<String>,
    pub status: Option<AdminUserStatus>,
    pub limit: i64,
    pub offset: i64,
}

impl AdminUserListRequest {
    /// 校验筛选文本和有限数字分页。
    pub fn validate(&self) -> bool {
        valid_optional_text(self.name.as_deref(), 256)
            && self.limit > 0
            && self.limit <= MAX_ADMIN_USER_PAGE_SIZE
            && self.offset >= 0
            && self.offset <= MAX_ADMIN_USER_OFFSET
    }
}

/// Admin 用户列表页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUserPage {
    pub users: Vec<AdminUserListItem>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminUserPage {
    /// 校验筛选命中、页大小和 username/id 稳定排序。
    pub fn validate(&self, request: &AdminUserListRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && self.users.len() as i64 <= self.limit
            && self.users.iter().enumerate().all(|(index, user)| {
                user.validate()
                    && request.name.as_deref().is_none_or(|name| {
                        let needle = name.to_lowercase();
                        user.user.username.to_lowercase().contains(&needle)
                            || user
                                .user
                                .game_player_id
                                .as_deref()
                                .is_some_and(|value| value.to_lowercase().contains(&needle))
                    })
                    && request.status.is_none_or(|status| user.user.status == status)
                    && self.users.get(index + 1).is_none_or(|next| {
                        user.user.username.to_lowercase() < next.user.username.to_lowercase()
                            || user.user.username.eq_ignore_ascii_case(&next.user.username)
                                && user.user.id < next.user.id
                    })
            })
    }
}

/// 用户管理命令闭集，对应旧版 user_admin_commands.command_kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminUserCommandKind {
    Create,
    Rename,
    SetGamePlayerId,
    RevokeKey,
    ReissueKey,
    Blacklist,
    Unblacklist,
    RenameRevert,
}

impl AdminUserCommandKind {
    /// 返回迁移表和审计使用的稳定命令类型。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Rename => "rename",
            Self::SetGamePlayerId => "set_game_player_id",
            Self::RevokeKey => "revoke_key",
            Self::ReissueKey => "reissue_key",
            Self::Blacklist => "blacklist",
            Self::Unblacklist => "unblacklist",
            Self::RenameRevert => "rename_revert",
        }
    }
}

/// 创建用户时的 key 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminUserKeyMode {
    Manual,
    Generated,
}

impl AdminUserKeyMode {
    /// 返回请求摘要使用的稳定 key 来源。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Generated => "generated",
        }
    }
}

/// 创建用户命令；手工 key 只存在于当前命令的内存值对象中。
pub struct AdminUserCreateCommand {
    pub command_id: Uuid,
    pub username: String,
    pub game_player_id: String,
    pub key_mode: AdminUserKeyMode,
    pub manual_key: Option<MailLoginKey>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

impl fmt::Debug for AdminUserCreateCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminUserCreateCommand")
            .field("command_id", &self.command_id)
            .field("username", &self.username)
            .field("game_player_id", &self.game_player_id)
            .field("key_mode", &self.key_mode)
            .field("manual_key", &self.manual_key.as_ref().map(|_| "[REDACTED]"))
            .field("actor_id", &self.actor_id)
            .field("request_id", &self.request_id)
            .field("now", &self.now)
            .finish()
    }
}

/// 用户属性或 key 状态修改动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminUserMutationAction {
    Rename,
    SetGamePlayerId,
    RevokeKey,
    ReissueKey,
    Blacklist,
    Unblacklist,
}

impl AdminUserMutationAction {
    /// 转换为用户命令账本中的稳定类型。
    pub const fn kind(self) -> AdminUserCommandKind {
        match self {
            Self::Rename => AdminUserCommandKind::Rename,
            Self::SetGamePlayerId => AdminUserCommandKind::SetGamePlayerId,
            Self::RevokeKey => AdminUserCommandKind::RevokeKey,
            Self::ReissueKey => AdminUserCommandKind::ReissueKey,
            Self::Blacklist => AdminUserCommandKind::Blacklist,
            Self::Unblacklist => AdminUserCommandKind::Unblacklist,
        }
    }

    /// 返回命令摘要和数据库使用的稳定动作字符串。
    pub const fn as_str(self) -> &'static str {
        self.kind().as_str()
    }
}

/// 带 state_version fence 的用户修改命令。
#[derive(Debug, Clone)]
pub struct AdminUserMutationCommand {
    pub command_id: Uuid,
    pub user_id: Uuid,
    pub expected_state_version: i64,
    pub action: AdminUserMutationAction,
    pub username: Option<String>,
    pub game_player_id: Option<String>,
    pub public_reason: Option<String>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 改名回滚命令；回滚目标由 rename_id 绑定，版本 fence 保护用户当前状态。
#[derive(Debug, Clone)]
pub struct AdminRenameRevertCommand {
    pub command_id: Uuid,
    pub rename_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 管理命令写入的密文 key 材料；不含 key 明文。
pub struct AdminUserKeyWrite {
    pub public_key_hash: [u8; 32],
    pub envelope: MailLoginKeyEnvelope,
    pub response_envelope: Option<MailLoginKeyEnvelope>,
}

/// Users owner 创建写入。
pub struct AdminUserCreateWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub user_id: Uuid,
    pub username: String,
    pub game_player_id: String,
    pub key: AdminUserKeyWrite,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Users owner 修改写入。
pub struct AdminUserMutationWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub user_id: Uuid,
    pub expected_state_version: i64,
    pub action: AdminUserMutationAction,
    pub username: Option<String>,
    pub game_player_id: Option<String>,
    pub public_reason: Option<String>,
    pub key: Option<AdminUserKeyWrite>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// Users owner 改名回滚写入。
pub struct AdminRenameRevertWrite {
    pub command_id: Uuid,
    pub request_hash: RequestHash,
    pub rename_id: Uuid,
    pub expected_state_version: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub now: DateTime<Utc>,
}

/// 用户管理 owner 返回的安全结果；响应 key 是对应命令的历史密文信封。
pub struct AdminUserCommandResult {
    pub user: AdminUser,
    pub response_envelope: Option<MailLoginKeyEnvelope>,
}

impl AdminUserCommandResult {
    /// 校验结果投影与可选密文信封。
    pub fn validate(&self) -> bool {
        self.user.validate()
            && self.response_envelope.as_ref().is_none_or(|envelope| {
                envelope.ciphertext().len() >= 16
                    && envelope.nonce().len() == 12
                    && !envelope.encryption_key_id().is_empty()
                    && !envelope.fingerprint().is_empty()
            })
    }
}

/// Admin 用户用例返回；明文 key 只供 HTTP 适配器一次性消费。
pub struct AdminUserMutationResult {
    pub user: AdminUser,
    pub generated_key: Option<MailLoginKey>,
}

/// 用户管理业务错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminUserError {
    #[error("Admin 用户命令参数无效")]
    InvalidRequest,
    #[error("用户或改名记录不存在")]
    NotFound,
    #[error("用户身份或名称冲突")]
    Conflict,
    #[error("命令 ID 与已有请求负载冲突")]
    CommandConflict,
    #[error("用户状态版本冲突，当前版本为 {current_version}")]
    VersionConflict { current_version: i64 },
    #[error("登录 key 不可用")]
    KeyUnavailable,
    #[error("改名记录不可回滚")]
    RenameNotRevertable,
    #[error("Users owner 返回结果损坏")]
    StoredResultCorrupt,
}

/// Admin 用户 key 生成/封装端口。
pub trait AdminLoginKeyIssuer: Send + Sync {
    fn generate(&self, user_id: Uuid) -> ApplicationFuture<'_, AdminUserKeyIssue>;
    fn seal(&self, user_id: Uuid, key: MailLoginKey) -> ApplicationFuture<'_, AdminUserKeyIssue>;
}

/// Admin 命令重放使用的历史密文解封端口。
pub trait AdminLoginKeyOpener: Send + Sync {
    fn open(
        &self,
        user_id: Uuid,
        envelope: MailLoginKeyEnvelope,
    ) -> ApplicationFuture<'_, MailLoginKey>;
}

/// 生成或封装后的 key；明文只在应用服务短暂持有。
pub struct AdminUserKeyIssue {
    pub key: MailLoginKey,
    pub envelope: MailLoginKeyEnvelope,
}

impl fmt::Debug for AdminUserKeyIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Users owner 的 Admin 查询、幂等和原子写入端口。
pub trait AdminUserStore: Send + Sync {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage>;
    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminUserCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminUserCommandResult>>;
    fn create(&self, write: AdminUserCreateWrite) -> ApplicationFuture<'_, AdminUserCommandResult>;
    fn mutate(
        &self,
        write: AdminUserMutationWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult>;
    fn revert_rename(
        &self,
        write: AdminRenameRevertWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult>;
}

/// Users owner 的 Admin 查询与 mutation 端口。
pub trait AdminUserPort: Send + Sync {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage>;
    fn create(
        &self,
        command: AdminUserCreateCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
    fn mutate(
        &self,
        command: AdminUserMutationCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
    fn revert_rename(
        &self,
        command: AdminRenameRevertCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult>;
}

/// Users owner 的 recovery 买家黑名单窄端口。
///
/// 该端口不把 recovery 状态写入 Users 表；它只负责用户黑名单、黑名单历史、用户命令账本和
/// 审计事实，完成后由 recovery owner 以独立 proof 推进 action。
pub trait RecoveryBuyerBlacklister: Send + Sync {
    fn ensure_buyer_blacklisted(
        &self,
        command: RecoveryBuyerBlacklistCommand,
    ) -> ApplicationFuture<'_, RecoveryBuyerBlacklistResult>;
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| value.is_empty() || valid_text(value, maximum))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
