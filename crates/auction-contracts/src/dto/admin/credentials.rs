//! credentials 相关 DTO。

use super::super::shared::deserialize_required_option;
use super::*;

/// Admin 凭据导入请求；Cookie 与登录输入互斥，敏感字段只在本次请求内存在。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCreateRequest {
    pub command_id: Uuid,
    pub kind: String,
    pub world: String,
    pub label: String,
    /// 可选预期完整玩家名称，仅 mail_cookie 接受；不是身份证明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_name: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<AdminCredentialLoginRequest>,
}

impl std::fmt::Debug for AdminCredentialCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCredentialCreateRequest")
            .field("command_id", &self.command_id)
            .field("kind", &self.kind)
            .field("world", &self.world)
            .field("label", &self.label)
            .field("secret", &"[REDACTED]")
            .field("login", &self.login)
            .finish()
    }
}

/// 论坛账号登录输入；响应只包含导入后的凭据安全投影，不返回 Cookie。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialLoginRequest {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for AdminCredentialLoginRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Admin 凭据改名或状态修改请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialMutationRequest {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    pub label: Option<String>,
    pub status: Option<String>,
}

/// 已保存凭据的只读校验请求；不接受 Cookie、URL 或状态修改。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCheckRequest {
    pub expected_state_version: i64,
}

/// 一个范围的认证观测：status 仅为 valid/invalid/inconclusive。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCheckScopeResponse {
    pub world: String,
    pub status: String,
}

/// 瞬时检查结果；不持久化、不改变凭据 active/disabled/invalid 状态。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCheckResponse {
    pub credential_id: String,
    pub state_version: i64,
    pub checked_at: String,
    pub checks: Vec<AdminCredentialCheckScopeResponse>,
}

/// Admin 凭据删除请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialDeleteRequest {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 凭据安全响应；不含秘密及指纹，`rotated_at` 必须显式出现但可为 `null`。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialResponse {
    pub id: String,
    pub kind: String,
    pub world: String,
    pub label: String,
    #[serde(default)]
    pub player_name: Option<String>,
    pub status: String,
    pub state_version: i64,
    pub created_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub rotated_at: Option<String>,
    pub updated_at: String,
}

/// Admin 凭据列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 凭据列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialListResponse {
    pub credentials: Vec<AdminCredentialResponse>,
    pub pagination: AdminCredentialPaginationResponse,
    pub restart_required: bool,
}

/// Admin 凭据导入/修改响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialMutationResponse {
    pub credential: AdminCredentialResponse,
    pub restart_required: bool,
}

/// Admin 凭据删除响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialDeleteResponse {
    pub credential_id: String,
    pub state_version: i64,
    pub restart_required: bool,
}
