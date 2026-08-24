//! Admin 凭据管理 API 客户端。
//!
//! 浏览器只保存凭据安全投影。导入表单的 secret 仅用于当前请求，成功后清空本地状态，
//! 列表和命令响应都不包含凭据内容。

use auction_contracts::{
    AdminCredentialCreateRequest, AdminCredentialDeleteRequest, AdminCredentialListResponse,
    AdminCredentialMutationRequest, AdminCredentialMutationResponse,
};
pub(crate) use auction_contracts::{
    AdminCredentialDeleteResponse as AdminCredentialDeleteResult,
    AdminCredentialResponse as AdminCredential,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{ApiResponse, fetch_csrf_token};

/// 凭据列表页的浏览器展示投影；分页字段在这里展平，wire 形状仍由 contracts 拥有。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminCredentialPage {
    pub credentials: Vec<AdminCredential>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub restart_required: bool,
}

impl From<AdminCredentialListResponse> for AdminCredentialPage {
    fn from(data: AdminCredentialListResponse) -> Self {
        Self {
            credentials: data.credentials,
            page: data.pagination.page,
            page_size: data.pagination.page_size,
            total: data.pagination.total,
            restart_required: data.restart_required,
        }
    }
}

/// 获取带筛选和页码的 Admin 凭据安全列表。
pub(crate) async fn fetch_admin_credentials(
    kind: String,
    world: String,
    status: String,
    page: i64,
) -> Result<AdminCredentialPage, String> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string()).append_pair("page_size", "50");
    if !kind.is_empty() {
        query.append_pair("kind", kind.trim());
    }
    if !world.is_empty() {
        query.append_pair("world", world.trim());
    }
    if !status.is_empty() {
        query.append_pair("status", status.trim());
    }
    let url = format!("/api/v1/admin/credentials?{}", query.finish());
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminCredentialListResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

/// 导入一条凭据；secret 不会被写入前端状态或响应模型。
pub(crate) async fn create_admin_credential(
    command_id: Uuid,
    kind: String,
    world: String,
    label: String,
    secret: String,
) -> Result<AdminCredential, String> {
    let csrf_token = fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/credentials")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminCredentialCreateRequest { command_id, kind, world, label, secret })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.credential)
}

/// 修改凭据标签或状态。
pub(crate) async fn mutate_admin_credential(
    command_id: Uuid,
    credential: &AdminCredential,
    action: &'static str,
    label: Option<String>,
    status: Option<String>,
) -> Result<AdminCredential, String> {
    let credential_id =
        Uuid::parse_str(&credential.id).map_err(|_| "invalid credential id".to_owned())?;
    let csrf_token = fetch_csrf_token().await?;
    let request = Request::patch("/api/v1/admin/credentials")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminCredentialMutationRequest {
            command_id,
            credential_id,
            expected_state_version: credential.state_version,
            action: action.to_owned(),
            label,
            status,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.credential)
}

/// 删除凭据。
pub(crate) async fn delete_admin_credential(
    command_id: Uuid,
    credential: &AdminCredential,
) -> Result<AdminCredentialDeleteResult, String> {
    let credential_id =
        Uuid::parse_str(&credential.id).map_err(|_| "invalid credential id".to_owned())?;
    let csrf_token = fetch_csrf_token().await?;
    let request = Request::delete("/api/v1/admin/credentials")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminCredentialDeleteRequest {
            command_id,
            credential_id,
            expected_state_version: credential.state_version,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminCredentialDeleteResult> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
