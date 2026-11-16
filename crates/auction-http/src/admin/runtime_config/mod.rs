//! Admin runtime 配置 HTTP 适配器。
//! HTTP 只负责 session/CSRF、稳定 JSON DTO 和错误状态映射；非敏感配置文档的严格校验、
//! fingerprint fence、文件原子替换和命令账本重放由应用端口及其外层适配器负责。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminRuntimeAlertsConfig, AdminRuntimeCommandError, AdminRuntimeConfigError,
    AdminRuntimeDocument, AdminRuntimeFileError, AdminRuntimeMailConfig, AdminRuntimeSnapshot,
    AdminRuntimeUpdateCommand,
};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
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

use crate::diagnostics;
use crate::errors::{CAPABILITY_NOT_PROVIDED, api_error, feature_unavailable};
use crate::middleware::boundary::request_id;
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session, internal_error,
    no_store_headers,
};
use crate::state::AdminApiState;

mod taxonomy;

/// runtime 端口只在角色配置引用 `runtime_config_file` 时装配；提示指向缺少的文件配置。
const RUNTIME_CONFIG_FILE_MISSING: &str = "未配置 runtime_config_file，runtime 配置不可用";

/// 读取当前 runtime TOML 的安全文档和 fingerprint。
pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let include_taxonomy = match taxonomy_projection(&uri) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable(RUNTIME_CONFIG_FILE_MISSING);
    };
    let snapshot = match port.current().await {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot, include_taxonomy) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        no_store_headers(),
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
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    let include_taxonomy = match taxonomy_projection(&uri) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let input: AdminRuntimeConfigValidateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let document = match parse_document(input.document) {
        Ok(document) => document,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable(RUNTIME_CONFIG_FILE_MISSING);
    };
    let snapshot = match port.validate(document) {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot, include_taxonomy) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        no_store_headers(),
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
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let include_taxonomy = match taxonomy_projection(&uri) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let input: AdminRuntimeConfigUpdateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
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
        return feature_unavailable(RUNTIME_CONFIG_FILE_MISSING);
    };
    let snapshot = match port
        .update(AdminRuntimeUpdateCommand {
            command_id: CommandId::from_uuid(input.command_id),
            expected_fingerprint: input.expected_fingerprint,
            document,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot, include_taxonomy) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    (
        StatusCode::OK,
        no_store_headers(),
        axum::Json(ApiResponse::ok("runtime 配置已写入，重启后生效", response)),
    )
        .into_response()
}

fn parse_document(value: Value) -> Result<AdminRuntimeDocument, Box<Response>> {
    serde_json::from_value(value).map_err(|_| {
        Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_runtime_config", "runtime 配置文档无效")
                .into_response(),
        )
    })
}

fn map_snapshot(
    snapshot: AdminRuntimeSnapshot,
    include_taxonomy: bool,
) -> Result<AdminRuntimeConfigResponse, Box<Response>> {
    snapshot.validate().map_err(|diagnostic_cause| {
        diagnostics::error(
            "admin_runtime",
            "validate_snapshot",
            diagnostics::invalid_state(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        Box::new(internal_error())
    })?;
    let mut document = snapshot.document;
    // 升级前的 Rust runtime 文件没有旧版三组业务字段。HTTP 投影补齐旧版默认值，使管理端
    // 能立即编辑完整契约；物理文件在首次成功保存前仍保持原指纹，避免破坏 durable 重放。
    document.materialize_business_defaults();
    let equipment_taxonomy = include_taxonomy.then(|| taxonomy::equipment_taxonomy(&document));
    let mut document = serde_json::to_value(document).map_err(|diagnostic_cause| {
        diagnostics::error(
            "admin_runtime",
            "encode_document",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        Box::new(internal_error())
    })?;
    // `false` 在物理文档中省略以保持旧指纹；浏览器投影显式补齐开关，避免隐藏默认行为。
    let Some(equipment_source) =
        document.get_mut("equipment_source").and_then(Value::as_object_mut)
    else {
        return Err(Box::new(internal_error()));
    };
    equipment_source.entry("require_tradeable").or_insert(Value::Bool(false));
    // 邮件区块的默认值在物理文档中省略，以保持旧文档指纹与命令摘要；浏览器投影补齐全部省略键，
    // 管理端据此严格校验完整闭集，不再自备默认值。显式值优先，补齐不改变候选的规范序列化。
    if let Some(mail) = document
        .get_mut("operations")
        .and_then(Value::as_object_mut)
        .and_then(|operations| operations.get_mut("mail"))
        .and_then(Value::as_object_mut)
    {
        let defaults = AdminRuntimeMailConfig::omitted_defaults().map_err(|diagnostic_cause| {
            diagnostics::error(
                "admin_runtime",
                "encode_mail_defaults",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            Box::new(internal_error())
        })?;
        for (key, value) in defaults {
            mail.entry(key).or_insert(value);
        }
    }
    // 告警区块新增阈值同样按默认值省略；浏览器投影补齐，管理端按完整闭集校验。
    if let Some(alerts) = document
        .get_mut("operations")
        .and_then(Value::as_object_mut)
        .and_then(|operations| operations.get_mut("alerts"))
        .and_then(Value::as_object_mut)
    {
        for (key, value) in AdminRuntimeAlertsConfig::omitted_defaults() {
            alerts.entry(key).or_insert(value);
        }
    }
    Ok(AdminRuntimeConfigResponse {
        document,
        fingerprint: snapshot.fingerprint,
        restart_required: snapshot.restart_required,
        equipment_taxonomy,
    })
}

// 词典仅扩展明确选择的响应投影；旧客户端仍收到原字段集合，查询不参与文档或命令摘要。
fn taxonomy_projection(uri: &Uri) -> Result<bool, Box<Response>> {
    let invalid = || {
        Box::new(
            api_error(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                "runtime 配置只接受 include_equipment_taxonomy=true",
            )
            .into_response(),
        )
    };
    let query = parse_query(uri, &["include_equipment_taxonomy"]).map_err(|()| invalid())?;
    match query.get("include_equipment_taxonomy").map(String::as_str) {
        None => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(invalid()),
    }
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminRuntime(
            AdminRuntimeConfigError::InvalidDocument(_)
            | AdminRuntimeConfigError::InvalidCommand(_),
        ) => api_error(StatusCode::BAD_REQUEST, "invalid_runtime_config", "runtime 配置文档无效")
            .into_response(),
        ApplicationError::AdminRuntime(AdminRuntimeConfigError::StaleFingerprint)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::CommandConflict)
        | ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::Conflict)
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Conflict) => {
            api_error(StatusCode::CONFLICT, "state_conflict", "runtime 配置版本或命令已发生变化")
                .into_response()
        }
        ApplicationError::AdminRuntime(AdminRuntimeConfigError::OutcomeUnknown)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::ReconciliationRequired)
        | ApplicationError::AdminRuntime(AdminRuntimeConfigError::StoredResultCorrupt)
        | ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::OutcomeUnknown) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "reconciliation_required",
            "runtime 配置更新结果需要人工核对",
        )
        .into_response(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(CAPABILITY_NOT_PROVIDED),
        ApplicationError::Serialization(_) => internal_error(),
        ApplicationError::AdminRuntimeFile(AdminRuntimeFileError::Unavailable(_))
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::NotFound)
        | ApplicationError::AdminRuntimeCommand(AdminRuntimeCommandError::Failed(_)) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "runtime 配置存储暂不可用",
        )
        .into_response(),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "runtime 配置服务暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_runtime",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

#[cfg(test)]
mod tests;
