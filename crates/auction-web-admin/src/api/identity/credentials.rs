//! Admin 凭据管理 API 客户端。
//!
//! 浏览器只保存凭据安全投影。导入表单的 secret 仅用于当前请求，成功后清空本地状态，
//! 列表和命令响应都不包含凭据内容。

use auction_contracts::{
    AdminCredentialCheckRequest, AdminCredentialCreateRequest, AdminCredentialDeleteRequest,
    AdminCredentialDeleteResponse, AdminCredentialListResponse, AdminCredentialMutationRequest,
    AdminCredentialMutationResponse, AdminCredentialPaginationResponse,
};
pub(crate) use auction_contracts::{
    AdminCredentialCheckResponse as AdminCredentialCheck,
    AdminCredentialDeleteResponse as AdminCredentialDeleteResult,
    AdminCredentialResponse as AdminCredential,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, TABLE_PAGE_SIZE, fetch_csrf_token, invalid_request_error,
    invalid_response_error, response_error, send_get, send_request, valid_admin_time,
    valid_trimmed_text, valid_uuid,
};

// 本地 Strict 镜像已删除：contracts 的 `AdminCredentialResponse`（`rotated_at` 保持必填
// nullable 语义）、`AdminCredentialListResponse`/`AdminCredentialMutationResponse`/
// `AdminCredentialDeleteResponse` 均已带 `deny_unknown_fields`；本文件只保留筛选回显、
// restart_required 与版本递增等跨字段谓词。

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

/// 只读检查指定版本；响应的世界、状态与时间必须通过闭集校验。
pub(crate) async fn check_admin_credential(
    credential: &AdminCredential,
) -> Result<AdminCredentialCheck, String> {
    decode_credential(credential.clone()).map_err(invalid_request_error)?;
    let credential_id = Uuid::parse_str(&credential.id).map_err(invalid_request_error)?;
    let csrf_token = fetch_csrf_token().await?;
    let request = AdminCredentialCheckRequest { expected_state_version: credential.state_version };
    let response = send_request(|abort_signal| {
        Request::post(&format!("/api/v1/admin/credentials/{credential_id}/check"))
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialCheck> =
        response.json().await.map_err(invalid_response_error)?;
    let result = payload.data;
    let worlds: &[&str] = match (credential.kind.as_str(), credential.world.as_str()) {
        ("forum_cookie", _) => &["shared"],
        ("mail_cookie", "main") => &["main"],
        ("mail_cookie", "isekai") => &["isekai"],
        ("mail_cookie", "shared") => &["main", "isekai"],
        _ => return Err("invalid_request".to_owned()),
    };
    if result.credential_id != credential.id
        || result.state_version != credential.state_version
        || !valid_admin_time(&result.checked_at)
        || result.checks.len() != worlds.len()
        || result.checks.iter().zip(worlds).any(|(check, world)| {
            check.world != *world
                || !matches!(check.status.as_str(), "valid" | "invalid" | "inconclusive")
        })
    {
        return Err("invalid_response".to_owned());
    }
    Ok(result)
}

/// 获取带筛选和页码的 Admin 凭据安全列表。
pub(crate) async fn fetch_admin_credentials(
    kind: String,
    world: String,
    status: String,
    page: i64,
) -> Result<AdminCredentialPage, String> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("page", &page.max(1).to_string())
        .append_pair("page_size", &TABLE_PAGE_SIZE.to_string());
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
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_credential_list(payload.data, kind.trim(), world.trim(), status.trim(), page.max(1))
        .map(AdminCredentialPage::from)
        .map_err(invalid_response_error)
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
    let command = AdminCredentialCreateRequest { command_id, kind, world, label, secret };
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/credentials")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let data = decode_credential_data(payload.data).map_err(invalid_response_error)?;
    if data.credential.kind != command.kind
        || data.credential.world != command.world
        || data.credential.label != command.label
    {
        return Err("invalid_response".to_owned());
    }
    Ok(data.credential)
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
        Uuid::parse_str(&credential.id).map_err(|_| "invalid_request".to_owned())?;
    let csrf_token = fetch_csrf_token().await?;
    let command = AdminCredentialMutationRequest {
        command_id,
        credential_id,
        expected_state_version: credential.state_version,
        action: action.to_owned(),
        label,
        status,
    };
    let response = send_request(|abort_signal| {
        Request::patch("/api/v1/admin/credentials")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialMutationResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let data = decode_credential_data(payload.data).map_err(invalid_response_error)?;
    let updated = data.credential;
    if updated.id != command.credential_id.to_string()
        || Some(updated.state_version) != command.expected_state_version.checked_add(1)
        || updated.kind != credential.kind
        || updated.world != credential.world
        || match command.action.as_str() {
            "rename" => {
                command.label.as_deref() != Some(updated.label.as_str())
                    || command.status.is_some()
                    || updated.status != credential.status
            }
            "set_status" => {
                command.status.as_deref() != Some(updated.status.as_str())
                    || command.label.is_some()
                    || updated.label != credential.label
            }
            _ => true,
        }
    {
        return Err("invalid_response".to_owned());
    }
    Ok(updated)
}

/// 删除凭据。
pub(crate) async fn delete_admin_credential(
    command_id: Uuid,
    credential: &AdminCredential,
) -> Result<AdminCredentialDeleteResult, String> {
    let credential_id =
        Uuid::parse_str(&credential.id).map_err(|_| "invalid_request".to_owned())?;
    let csrf_token = fetch_csrf_token().await?;
    let command = AdminCredentialDeleteRequest {
        command_id,
        credential_id,
        expected_state_version: credential.state_version,
    };
    let response = send_request(|abort_signal| {
        Request::delete("/api/v1/admin/credentials")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminCredentialDeleteResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_credential_delete(
        payload.data,
        &command.credential_id.to_string(),
        command.expected_state_version,
    )
    .map_err(invalid_response_error)
}

fn decode_credential_list(
    data: AdminCredentialListResponse,
    kind_filter: &str,
    world_filter: &str,
    status_filter: &str,
    requested_page: i64,
) -> Result<AdminCredentialListResponse, &'static str> {
    if !data.restart_required {
        return Err("credential_restart");
    }
    decode_credential_pagination(&data.pagination)?;
    if data.pagination.page != requested_page
        || data.pagination.page_size != TABLE_PAGE_SIZE
        || i64::try_from(data.credentials.len()).map_or(true, |count| {
            count > data.pagination.page_size || count > data.pagination.total
        })
    {
        return Err("credential_pagination");
    }
    let credentials =
        data.credentials.into_iter().map(decode_credential).collect::<Result<Vec<_>, _>>()?;
    if credentials.iter().any(|credential| {
        (!kind_filter.is_empty() && credential.kind != kind_filter)
            || (!world_filter.is_empty() && credential.world != world_filter)
            || (!status_filter.is_empty() && credential.status != status_filter)
    }) {
        return Err("credential_filter");
    }
    Ok(AdminCredentialListResponse {
        credentials,
        pagination: data.pagination,
        restart_required: true,
    })
}

fn decode_credential_data(
    data: AdminCredentialMutationResponse,
) -> Result<AdminCredentialMutationResponse, &'static str> {
    if !data.restart_required {
        return Err("credential_restart");
    }
    Ok(AdminCredentialMutationResponse {
        credential: decode_credential(data.credential)?,
        restart_required: true,
    })
}

fn decode_credential_delete(
    data: AdminCredentialDeleteResponse,
    requested_credential_id: &str,
    expected_state_version: i64,
) -> Result<AdminCredentialDeleteResult, &'static str> {
    if !data.restart_required
        || !valid_uuid(&data.credential_id)
        || data.credential_id != requested_credential_id
        || Some(data.state_version) != expected_state_version.checked_add(1)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&data.state_version)
    {
        return Err("credential_delete");
    }
    Ok(AdminCredentialDeleteResult {
        credential_id: data.credential_id,
        state_version: data.state_version,
        restart_required: true,
    })
}

/// 跨字段谓词保留为独立校验：类型/世界/状态枚举与值域仍在浏览器边界 fail-closed。
fn decode_credential(value: AdminCredential) -> Result<AdminCredential, &'static str> {
    if !valid_uuid(&value.id)
        || !matches!(value.kind.as_str(), "forum_cookie" | "mail_cookie")
        || !matches!(value.world.as_str(), "main" | "isekai" | "shared")
        || !valid_trimmed_text(&value.label)
        || value.label.len() > 256
        || !matches!(value.status.as_str(), "active" | "disabled" | "invalid")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || value.rotated_at.as_deref().is_some_and(|time| !valid_admin_time(time))
        || !valid_admin_time(&value.updated_at)
    {
        return Err("credential");
    }
    Ok(value)
}

fn decode_credential_pagination(
    value: &AdminCredentialPaginationResponse,
) -> Result<(), &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("credential_pagination");
    }
    Ok(())
}
