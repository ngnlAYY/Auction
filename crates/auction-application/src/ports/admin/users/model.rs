//! Admin Users owner 的安全投影、列表请求和分页模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::common::{valid_optional_text, valid_text, valid_time};

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
