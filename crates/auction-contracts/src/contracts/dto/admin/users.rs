//! users 相关 DTO。

use super::super::shared::deserialize_null_default;
use super::*;

/// Admin 用户创建请求；`key` 仅在手工 key 模式提供，服务端不会把它写入日志或命令账本。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserCreateRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub username: String,
    pub forum_account_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub key_mode: String,
    pub key: Option<String>,
}

impl std::fmt::Debug for AdminUserCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminUserCreateRequest")
            .field("command_id", &self.command_id)
            .field("username", &self.username)
            .field("forum_account_id", &self.forum_account_id)
            .field("key_mode", &self.key_mode)
            .field("key", &self.key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserMutationRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub action: String,
    pub username: Option<String>,
    pub forum_account_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserBlacklistRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub public_reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUserUnblacklistRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
}

/// Admin 改名回滚请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRenameRevertRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
}

/// Admin 用户安全响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserResponse {
    pub id: String,
    pub username: String,
    pub forum_account_id: Option<String>,
    pub status: String,
    pub key_status: String,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

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

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserListResponse {
    pub users: Vec<AdminUserListItemResponse>,
    pub pagination: AdminUserPaginationResponse,
}

/// Admin 用户修改响应；生成 key 只在本次创建/重签响应中出现，调试输出必须脱敏。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUserMutationResponse {
    pub user: AdminUserResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_key: Option<String>,
}

impl std::fmt::Debug for AdminUserMutationResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminUserMutationResponse")
            .field("user", &self.user)
            .field("generated_key", &self.generated_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}
