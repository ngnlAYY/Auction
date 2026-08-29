//! Admin 凭据管理 API 客户端。
//!
//! 浏览器只保存凭据安全投影。导入表单的 secret 仅用于当前请求，成功后清空本地状态，
//! 列表和命令响应都不包含凭据内容。

use auction_contracts::{
    AdminCredentialCreateRequest, AdminCredentialDeleteRequest, AdminCredentialListResponse,
    AdminCredentialMutationRequest, AdminCredentialMutationResponse,
    AdminCredentialPaginationResponse,
};
pub(crate) use auction_contracts::{
    AdminCredentialDeleteResponse as AdminCredentialDeleteResult,
    AdminCredentialResponse as AdminCredential,
};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, RequiredOption, TABLE_PAGE_SIZE, fetch_csrf_token,
    invalid_request_error, invalid_response_error, response_error, send_get, send_request,
    valid_admin_time, valid_trimmed_text, valid_uuid,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredential {
    created_at: String,
    id: String,
    kind: String,
    label: String,
    rotated_at: RequiredOption<String>,
    state_version: i64,
    status: String,
    updated_at: String,
    world: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredentialPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredentialList {
    credentials: Vec<StrictCredential>,
    pagination: StrictCredentialPagination,
    restart_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredentialData {
    credential: StrictCredential,
    restart_required: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictCredentialDeleteData {
    credential_id: String,
    restart_required: bool,
    state_version: i64,
}

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
    let payload: ApiResponse<StrictCredentialList> =
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
    let payload: ApiResponse<StrictCredentialData> =
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
    let payload: ApiResponse<StrictCredentialData> =
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
    let payload: ApiResponse<StrictCredentialDeleteData> =
        response.json().await.map_err(invalid_response_error)?;
    decode_credential_delete(
        payload.data,
        &command.credential_id.to_string(),
        command.expected_state_version,
    )
    .map_err(invalid_response_error)
}

fn decode_credential_list(
    data: StrictCredentialList,
    kind_filter: &str,
    world_filter: &str,
    status_filter: &str,
    requested_page: i64,
) -> Result<AdminCredentialListResponse, &'static str> {
    if !data.restart_required {
        return Err("credential_restart");
    }
    let pagination = decode_credential_pagination(data.pagination)?;
    if pagination.page != requested_page
        || pagination.page_size != TABLE_PAGE_SIZE
        || i64::try_from(data.credentials.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
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
    Ok(AdminCredentialListResponse { credentials, pagination, restart_required: true })
}

fn decode_credential_data(
    data: StrictCredentialData,
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
    data: StrictCredentialDeleteData,
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

fn decode_credential(value: StrictCredential) -> Result<AdminCredential, &'static str> {
    if !valid_uuid(&value.id)
        || !matches!(value.kind.as_str(), "forum_cookie" | "mail_cookie")
        || !matches!(value.world.as_str(), "main" | "isekai" | "shared")
        || !valid_trimmed_text(&value.label)
        || value.label.len() > 256
        || !matches!(value.status.as_str(), "active" | "disabled" | "invalid")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || value.rotated_at.0.as_deref().is_some_and(|time| !valid_admin_time(time))
        || !valid_admin_time(&value.updated_at)
    {
        return Err("credential");
    }
    Ok(AdminCredential {
        id: value.id,
        kind: value.kind,
        world: value.world,
        label: value.label,
        status: value.status,
        state_version: value.state_version,
        created_at: value.created_at,
        rotated_at: value.rotated_at.0,
        updated_at: value.updated_at,
    })
}

fn decode_credential_pagination(
    value: StrictCredentialPagination,
) -> Result<AdminCredentialPaginationResponse, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("credential_pagination");
    }
    Ok(AdminCredentialPaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}
