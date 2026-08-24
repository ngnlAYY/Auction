//! Admin runtime 配置 HTTP 适配器。
//!
//! HTTP 只负责 session/CSRF、稳定 JSON DTO 和错误状态映射；非敏感配置文档的严格校验、
//! fingerprint fence、文件原子替换和命令账本重放由应用端口及其外层适配器负责。

use std::net::SocketAddr;

use auction_application::{
    AdminRuntimeCommandError, AdminRuntimeConfigError, AdminRuntimeDocument, AdminRuntimeFileError,
    AdminRuntimeSnapshot, AdminRuntimeUpdateCommand, ApplicationError,
};
use auction_contracts::{
    AdminRuntimeConfigResponse, AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest,
    ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::Value;

use super::AdminApiState;

/// 读取当前 runtime TOML 的安全文档和 fingerprint。
pub(crate) async fn get(
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
    if super::parse_query(&uri, &[]).is_err() {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "runtime 配置不接受查询参数",
        )
        .into_response();
    }
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable();
    };
    let snapshot = match port.current().await {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("runtime 配置已加载", response)),
    )
        .into_response()
}

/// 只校验候选 runtime 文档，不修改 TOML 文件或命令账本。
pub(crate) async fn validate(
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
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let input: AdminRuntimeConfigValidateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let document = match parse_document(input.document) {
        Ok(document) => document,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable();
    };
    let snapshot = match port.validate(document) {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("runtime 配置候选已验证", response)),
    )
        .into_response()
}

/// 以当前 fingerprint 为 fence 原子替换 runtime TOML，并记录可恢复的 durable 命令。
pub(crate) async fn update(
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
    let input: AdminRuntimeConfigUpdateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let document = match parse_document(input.document) {
        Ok(document) => document,
        Err(response) => return *response,
    };
    let request_hash = match document.request_hash(&input.expected_fingerprint) {
        Ok(hash) => hash,
        Err(error) => return map_error(ApplicationError::from(error)),
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable();
    };
    let snapshot = match port
        .update(AdminRuntimeUpdateCommand {
            command_id: CommandId::from_uuid(input.command_id),
            expected_fingerprint: input.expected_fingerprint,
            document,
            actor_id: identity.actor,
            request_id: super::request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        axum::Json(ApiResponse::ok("runtime 配置已写入，重启后生效", response)),
    )
        .into_response()
}

fn parse_document(value: Value) -> Result<AdminRuntimeDocument, Box<Response>> {
    serde_json::from_value(value).map_err(|_| {
        Box::new(
            super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_runtime_config",
                "runtime 配置文档无效",
            )
            .into_response(),
        )
    })
}

fn map_snapshot(
    snapshot: AdminRuntimeSnapshot,
) -> Result<AdminRuntimeConfigResponse, Box<Response>> {
    snapshot.validate().map_err(|_| Box::new(super::internal_error()))?;
    let document =
        serde_json::to_value(snapshot.document).map_err(|_| Box::new(super::internal_error()))?;
    Ok(AdminRuntimeConfigResponse {
        document,
        fingerprint: snapshot.fingerprint,
        restart_required: snapshot.restart_required,
    })
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
            super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "runtime 配置命令不接受查询参数",
            )
            .into_response(),
        );
    }
    None
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "runtime 配置更新仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminRuntime(
            AdminRuntimeConfigError::InvalidDocument(_)
            | AdminRuntimeConfigError::InvalidCommand(_),
        ) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_runtime_config",
            "runtime 配置文档无效",
        )
        .into_response(),
        ApplicationError::AdminRuntime(AdminRuntimeConfigError::StaleFingerprint)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::CommandConflict)
        | ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::Conflict)
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Conflict) => {
            super::api_error(
                StatusCode::CONFLICT,
                "state_conflict",
                "runtime 配置版本或命令已发生变化",
            )
            .into_response()
        }
        ApplicationError::AdminRuntime(AdminRuntimeConfigError::OutcomeUnknown)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::ReconciliationRequired)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::StoredResultCorrupt)
        | ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::OutcomeUnknown) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "reconciliation_required",
                "runtime 配置更新结果需要人工核对",
            )
            .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::Unavailable(_))
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::NotFound)
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Failed(_)) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "runtime 配置存储暂不可用",
            )
            .into_response()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "runtime 配置服务暂不可用",
        )
        .into_response(),
    }
}
