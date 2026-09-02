//! Admin Users owner 的命令、写入载荷、恢复黑名单结果和业务错误。

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use auction_domain::SettlementId;

use super::super::RequestHash;
use super::super::user_mail::{MailLoginKey, MailLoginKeyEnvelope};
use super::model::{
    AdminUser, AdminUserStatus, RECOVERY_BUYER_BLACKLIST_ACTOR,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON,
};

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
            && super::common::valid_time(self.occurred_at)
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
            && super::common::valid_time(self.occurred_at)
            && self.user.validate()
    }

    /// 按旧版 recovery buyer blacklist result 的 schema-v1 字段顺序生成证明摘要。
    ///
    /// Users owner 返回的完整结果才是证明事实；不能只按 action 与 command reference 生成替代摘要，
    /// 否则滚动升级接管时同一已完成黑名单会得到不同 proof。
    pub fn legacy_proof_digest(&self) -> Result<[u8; 32], serde_json::Error> {
        #[derive(Serialize)]
        struct LegacyRecoveryBuyerBlacklistProof<'a> {
            schema_version: u8,
            command_id: String,
            recovery_id: String,
            action_id: String,
            buyer_user_id: String,
            public_reason: &'a str,
            actor_id: &'a str,
            occurred_at: String,
        }

        let payload = LegacyRecoveryBuyerBlacklistProof {
            schema_version: 1,
            command_id: self.command_id.to_string(),
            recovery_id: self.recovery_id.as_uuid().to_string(),
            action_id: self.action_id.as_uuid().to_string(),
            buyer_user_id: self.buyer_user_id.to_string(),
            public_reason: &self.public_reason,
            actor_id: &self.actor_id,
            occurred_at: legacy_go_time(self.occurred_at),
        };
        Ok(RequestHash::from_legacy_go_json(&payload)?.as_bytes())
    }
}

/// 复现 Go `time.Time.MarshalJSON` 使用的 RFC3339Nano：纳秒位尾零全部移除。
fn legacy_go_time(value: DateTime<Utc>) -> String {
    let encoded = value.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let Some(without_zone) = encoded.strip_suffix('Z') else {
        return encoded;
    };
    let without_fraction_padding = without_zone.trim_end_matches('0').trim_end_matches('.');
    format!("{without_fraction_padding}Z")
}

/// Admin 用户命令闭集，对应旧版 user_admin_commands.command_kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminUserCommandKind {
    Create,
    Rename,
    /// 账本字符串沿用旧值，避免破坏既有幂等命令；业务语义已改为稳定论坛账号 ID。
    SetForumAccountId,
    RevokeKey,
    ReissueKey,
    Blacklist,
    Unblacklist,
    RenameRevert,
}

impl AdminUserCommandKind {
    /// 返回既有幂等账本与请求摘要使用的稳定命令类型。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Rename => "rename",
            Self::SetForumAccountId => "set_game_player_id",
            Self::RevokeKey => "revoke_key",
            Self::ReissueKey => "reissue_key",
            Self::Blacklist => "blacklist",
            Self::Unblacklist => "unblacklist",
            Self::RenameRevert => "rename_revert",
        }
    }

    /// 返回面向审计和运维展示的当前业务动作名。
    pub const fn audit_str(self) -> &'static str {
        match self {
            Self::SetForumAccountId => "set_forum_account_id",
            _ => self.as_str(),
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
    pub forum_account_id: Option<String>,
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
            .field("forum_account_id", &self.forum_account_id)
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
    SetForumAccountId,
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
            Self::SetForumAccountId => AdminUserCommandKind::SetForumAccountId,
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
    pub forum_account_id: Option<String>,
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
    pub forum_account_id: Option<String>,
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
    pub forum_account_id: Option<String>,
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
