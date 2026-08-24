//! credentials 相关 DTO。

use super::*;

/// Admin 凭据导入请求；secret 只在请求体和本次加密调用中存在，Debug 实现必须脱敏。
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialCreateRequest {
    pub command_id: Uuid,
    pub kind: String,
    pub world: String,
    pub label: String,
    pub secret: String,
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
            .finish()
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

/// Admin 凭据删除请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCredentialDeleteRequest {
    pub command_id: Uuid,
    pub credential_id: Uuid,
    pub expected_state_version: i64,
}

/// Admin 凭据安全响应，不包含 secret、密文、密钥标识或指纹。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialResponse {
    pub id: String,
    pub kind: String,
    pub world: String,
    pub label: String,
    pub status: String,
    pub state_version: i64,
    pub created_at: String,
    pub rotated_at: Option<String>,
    pub updated_at: String,
}

/// Admin 凭据列表分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 凭据列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialListResponse {
    pub credentials: Vec<AdminCredentialResponse>,
    pub pagination: AdminCredentialPaginationResponse,
    pub restart_required: bool,
}

/// Admin 凭据导入/修改响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialMutationResponse {
    pub credential: AdminCredentialResponse,
    pub restart_required: bool,
}

/// Admin 凭据删除响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminCredentialDeleteResponse {
    pub credential_id: String,
    pub state_version: i64,
    pub restart_required: bool,
}
