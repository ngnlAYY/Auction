//! Admin 凭据管理 HTTP 适配器。
//!
//! 这里负责旧版 A32 的 session/CSRF、分页查询和 DTO 映射；密封、命令幂等、版本 fence、
//! 审计及 SQLite 能力边界由应用与 storage owner 负责。secret 只被转交给应用值对象，
//! 不进入响应、日志或错误文本。

use std::net::SocketAddr;

use auction_application::{
    AdminCredentialDeleteCommand, AdminCredentialError, AdminCredentialImportCommand,
    AdminCredentialKind, AdminCredentialListRequest, AdminCredentialRenameCommand,
    AdminCredentialSecret, AdminCredentialStatus, AdminCredentialStatusCommand,
    AdminCredentialWorld, ApplicationError,
};
use auction_contracts::{
    AdminCredentialCreateRequest, AdminCredentialDeleteRequest, AdminCredentialDeleteResponse,
    AdminCredentialListResponse, AdminCredentialMutationRequest, AdminCredentialMutationResponse,
    AdminCredentialPaginationResponse, AdminCredentialResponse, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取凭据安全投影列表。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(&uri, &["kind", "world", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return invalid_query(),
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page_size =
        match super::parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
            Ok(value) => value,
            Err(_) => return invalid_query(),
        };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(value) if value <= auction_application::MAX_ADMIN_CREDENTIAL_OFFSET => value,
        _ => return invalid_query(),
    };
    let kind = match query.get("kind").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialKind::parse(value) {
            Some(kind) => Some(kind),
            None => return invalid_query(),
        },
    };
    let world = match query.get("world").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialWorld::parse(value) {
            Some(world) => Some(world),
            None => return invalid_query(),
        },
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialStatus::parse(value) {
            Some(status) => Some(status),
            None => return invalid_query(),
        },
    };
    let request = AdminCredentialListRequest { kind, world, status, limit: page_size, offset };
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }
    let response = AdminCredentialListResponse {
        credentials: result.credentials.into_iter().map(map_credential).collect(),
        pagination: AdminCredentialPaginationResponse { page, page_size, total: result.total },
        restart_required: true,
    };
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("凭据列表已加载", response)))
        .into_response()
}

/// 导入新的 mail_cookie/forum_cookie。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminCredentialCreateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 无效");
    }
    let kind = if input.kind.is_empty() {
        AdminCredentialKind::MailCookie
    } else {
        match AdminCredentialKind::parse(&input.kind) {
            Some(kind) => kind,
            None => return invalid_request("invalid_credential_kind", "凭据类型无效"),
        }
    };
    let world = match AdminCredentialWorld::parse(&input.world) {
        Some(world) => world,
        None => return invalid_request("invalid_credential_world", "凭据 world 无效"),
    };
    let secret = match AdminCredentialSecret::new(input.secret) {
        Ok(secret) => secret,
        Err(_) => return invalid_request("invalid_credential_secret", "凭据内容无效"),
    };
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port
        .import(AdminCredentialImportCommand {
            command_id: input.command_id,
            kind,
            world,
            label: input.label,
            secret,
            actor_id: identity.actor,
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::CREATED,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已导入",
            AdminCredentialMutationResponse {
                credential: map_credential(result),
                restart_required: true,
            },
        )),
    )
        .into_response()
}

/// 修改凭据标签或 active/disabled 状态。
pub(crate) async fn mutate(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminCredentialMutationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() || input.credential_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 或凭据 ID 无效");
    }
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let credential = match input.action.as_str() {
        "rename" => {
            let Some(label) = input.label else {
                return invalid_request("invalid_credential_label", "凭据标签缺失");
            };
            if input.status.is_some() {
                return invalid_request("invalid_credential_action", "改名命令不接受 status");
            }
            match port
                .rename(AdminCredentialRenameCommand {
                    command_id: input.command_id,
                    credential_id: input.credential_id,
                    expected_state_version: input.expected_state_version,
                    label,
                    actor_id: identity.actor,
                    request_id: None,
                    now: Utc::now(),
                })
                .await
            {
                Ok(result) => result,
                Err(error) => return map_error(error),
            }
        }
        "set_status" => {
            let Some(status) = input.status.as_deref().and_then(AdminCredentialStatus::parse)
            else {
                return invalid_request("invalid_credential_status", "凭据状态无效");
            };
            if input.label.is_some() {
                return invalid_request("invalid_credential_action", "状态命令不接受 label");
            }
            match port
                .set_status(AdminCredentialStatusCommand {
                    command_id: input.command_id,
                    credential_id: input.credential_id,
                    expected_state_version: input.expected_state_version,
                    status,
                    actor_id: identity.actor,
                    request_id: None,
                    now: Utc::now(),
                })
                .await
            {
                Ok(result) => result,
                Err(error) => return map_error(error),
            }
        }
        _ => return invalid_request("invalid_credential_action", "凭据动作无效"),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已更新",
            AdminCredentialMutationResponse {
                credential: map_credential(credential),
                restart_required: true,
            },
        )),
    )
        .into_response()
}

/// 删除一条凭据。
pub(crate) async fn delete(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminCredentialDeleteRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() || input.credential_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 或凭据 ID 无效");
    }
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port
        .delete(AdminCredentialDeleteCommand {
            command_id: input.command_id,
            credential_id: input.credential_id,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已删除",
            AdminCredentialDeleteResponse {
                credential_id: result.credential_id.to_string(),
                state_version: result.state_version,
                restart_required: true,
            },
        )),
    )
        .into_response()
}

fn map_credential(credential: auction_application::AdminCredential) -> AdminCredentialResponse {
    AdminCredentialResponse {
        id: credential.id.to_string(),
        kind: credential.kind.as_str().to_owned(),
        world: credential.world.as_str().to_owned(),
        label: credential.label,
        status: credential.status.as_str().to_owned(),
        state_version: credential.state_version,
        created_at: super::timestamp(credential.created_at),
        rotated_at: credential.rotated_at.map(super::timestamp),
        updated_at: super::timestamp(credential.updated_at),
    }
}

fn mutation_boundary(headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "凭据命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "凭据管理只支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

fn invalid_json() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

fn invalid_request(code: &'static str, message: &'static str) -> Response {
    super::api_error(StatusCode::BAD_REQUEST, code, message).into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminCredential(AdminCredentialError::InvalidRequest) => {
            invalid_request("invalid_credential_request", "凭据管理参数无效")
        }
        ApplicationError::AdminCredential(AdminCredentialError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::Conflict)
        | ApplicationError::AdminCredential(AdminCredentialError::CommandConflict) => {
            super::api_error(
                StatusCode::CONFLICT,
                "credential_conflict",
                "凭据标签、指纹或命令负载与当前记录冲突",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::VersionConflict { .. }) => {
            super::api_error(
                StatusCode::CONFLICT,
                "stale_credential_state",
                "凭据状态版本已变化，请重新读取后再提交",
            )
            .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::AdminCredential(AdminCredentialError::CryptoUnavailable) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "credential_crypto_unavailable",
                "凭据加密能力暂不可用",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::StoredResultCorrupt) => {
            super::internal_error()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "凭据管理服务暂不可用",
        )
        .into_response(),
    }
}
