//! users 相关 DTO。

use super::*;

/// Admin 用户创建请求；`key` 仅在手工 key 模式提供，服务端不会把它写入日志或命令账本。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserCreateRequest {
    pub command_id: Uuid,
    pub username: String,
    pub game_player_id: String,
    pub key_mode: String,
    pub key: Option<String>,
}

impl std::fmt::Debug for AdminUserCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminUserCreateRequest")
            .field("command_id", &self.command_id)
            .field("username", &self.username)
            .field("game_player_id", &self.game_player_id)
            .field("key_mode", &self.key_mode)
            .field("key", &self.key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Admin 用户属性/key 状态修改请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserMutationRequest {
    pub command_id: Uuid,
    pub user_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    pub username: Option<String>,
    pub game_player_id: Option<String>,
    pub public_reason: Option<String>,
}

/// Admin 用户黑名单请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserBlacklistRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub public_reason: String,
}

/// Admin 用户解封请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserUnblacklistRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 改名回滚请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRenameRevertRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 用户安全响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub username: String,
    pub game_player_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Admin 用户列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 用户列表中的黑名单历史安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserBlacklistHistoryResponse {
    pub public_reason: String,
    pub blacklisted_at: String,
}

/// Admin 用户列表条目；写命令响应仍只使用 `AdminUserResponse`。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserListItemResponse {
    #[serde(flatten)]
    pub user: AdminUserResponse,
    pub blacklist_count: i64,
    pub blacklist_history: Vec<AdminUserBlacklistHistoryResponse>,
}

/// Admin 用户列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserListResponse {
    pub users: Vec<AdminUserListItemResponse>,
    pub pagination: AdminUserPaginationResponse,
}

/// Admin 用户修改响应；生成 key 只在本次创建/重签响应中出现。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserMutationResponse {
    pub user: AdminUserResponse,
    pub generated_key: Option<String>,
}
