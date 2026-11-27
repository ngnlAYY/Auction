//! Admin runtime 配置 HTTP 适配器。
//! HTTP 只负责 session/CSRF、稳定 JSON DTO 和错误状态映射；非敏感配置文档的严格校验、
//! fingerprint fence、文件原子替换和命令账本重放由应用端口及其外层适配器负责。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminRuntimeAlertsConfig, AdminRuntimeCommandError, AdminRuntimeConfigError,
    AdminRuntimeDocument, AdminRuntimeFileError, AdminRuntimeMailConfig, AdminRuntimeSnapshot,
    AdminRuntimeUpdateCommand, WorkerRestartState,
};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{
    AdminRuntimeConfigResponse, AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest,
    AdminWorkerRestartStateDto, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::Value;

use crate::diagnostics;
use crate::errors::{CAPABILITY_NOT_PROVIDED, api_error, feature_unavailable};
use crate::middleware::boundary::request_id;
use crate::middleware::strict_json;
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session, internal_error,
    no_store_headers,
};
use crate::state::AdminApiState;

mod taxonomy;

/// runtime 端口只在角色配置引用 `runtime_config_file` 时装配；提示指向缺少的文件配置。
const RUNTIME_CONFIG_FILE_MISSING: &str = "未配置 runtime_config_file，runtime 配置不可用";

/// runtime 配置请求体上限；只在鉴权后由解码头强制执行。
const RUNTIME_CONFIG_BODY_LIMIT: usize = 4 << 20;

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
    let projection = match runtime_projection(&uri, true) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable(RUNTIME_CONFIG_FILE_MISSING);
    };
    let result = if projection.worker_restart_state {
        port.current_with_worker_restart_state().await.map(|(snapshot, state)| {
            let state = match state {
                WorkerRestartState::RestartRequired => AdminWorkerRestartStateDto::RestartRequired,
                WorkerRestartState::Consistent => AdminWorkerRestartStateDto::Consistent,
                WorkerRestartState::Unknown => AdminWorkerRestartStateDto::Unknown,
            };
            (snapshot, Some(state))
        })
    } else {
        port.current().await.map(|snapshot| (snapshot, None))
    };
    let (snapshot, worker_restart_state) = match result {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let mut response = match map_snapshot(snapshot, projection) {
        Ok(response) => response,
        Err(response) => return *response,
    };
    response.worker_restart_state = worker_restart_state;
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
    body: Body,
) -> Response {
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    let projection = match runtime_projection(&uri, false) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let input: AdminRuntimeConfigValidateRequest =
        match strict_json::decode_after_guard(&headers, body, RUNTIME_CONFIG_BODY_LIMIT).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let document = match parse_document(input.document) {
        Ok(document) => document,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_runtime() else {
        return feature_unavailable(RUNTIME_CONFIG_FILE_MISSING);
    };
    let snapshot = match port.validate(document).await {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    let response = match map_snapshot(snapshot, projection) {
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
    body: Body,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let projection = match runtime_projection(&uri, false) {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let input: AdminRuntimeConfigUpdateRequest =
        match strict_json::decode_after_guard(&headers, body, RUNTIME_CONFIG_BODY_LIMIT).await {
            Ok(input) => input,
            Err(response) => return *response,
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
    let response = match map_snapshot(snapshot, projection) {
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
    projection: RuntimeProjection,
) -> Result<AdminRuntimeConfigResponse, Box<Response>> {
    if snapshot.document.fingerprint() != snapshot.fingerprint {
        diagnostics::error(
            "admin_runtime",
            "validate_snapshot_fingerprint",
            diagnostics::invalid_state("runtime document fingerprint mismatch"),
            DiagnosticContext::default(),
        );
        return Err(Box::new(internal_error()));
    }
    // GET preserves stored values beyond current write-validation bounds; write paths still
    // validate candidates with AdminRuntimeDocument::validate.
    let mut document = snapshot.document;
    // 升级前的 Rust runtime 文件没有旧版三组业务字段。HTTP 投影补齐旧版默认值，使管理端
    // 能立即编辑完整契约；物理文件在首次成功保存前仍保持原指纹，避免破坏 durable 重放。
    document.materialize_business_defaults();
    let equipment_taxonomy =
        projection.equipment_taxonomy.then(|| taxonomy::equipment_taxonomy(&document));
    let collection_overdue_age_seconds = document
        .operations
        .as_ref()
        .map(|operations| operations.alerts.collection_overdue_age_seconds());
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
    // 长期未领取阈值不属于已发布的告警区块闭集：缺省投影一律不输出（即使文件里写了非默认值），
    // 已打开的旧页面按字段数严格校验；新页面经 include 显式请求时给出生效值（省略即默认）。
    if let Some(alerts) = document
        .get_mut("operations")
        .and_then(Value::as_object_mut)
        .and_then(|operations| operations.get_mut("alerts"))
        .and_then(Value::as_object_mut)
    {
        alerts.remove(COLLECTION_OVERDUE_AGE_KEY);
        if projection.collection_overdue_threshold
            && let Some(seconds) = collection_overdue_age_seconds
        {
            alerts.insert(COLLECTION_OVERDUE_AGE_KEY.to_owned(), Value::from(seconds));
        }
    }
    Ok(AdminRuntimeConfigResponse {
        document,
        fingerprint: snapshot.fingerprint,
        restart_required: snapshot.restart_required,
        equipment_taxonomy,
        worker_restart_state: None,
    })
}

/// 告警区块里只经扩展投影输出的长期未领取阈值键；同时是 `include` 的唯一取值。
const COLLECTION_OVERDUE_AGE_KEY: &str = "fulfillment_collection_overdue_age_seconds";

/// runtime 配置响应投影；查询不参与文档或命令摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeProjection {
    /// `include_equipment_taxonomy=true`：附带装备分类词典。
    equipment_taxonomy: bool,
    /// `include=fulfillment_collection_overdue_age_seconds`：告警区块附带长期未领取阈值。
    collection_overdue_threshold: bool,
    /// `include_worker_restart_state=true`：读取此快照对应的 Worker 冷配置状态。
    worker_restart_state: bool,
}

// 只扩展明确选择的响应投影；旧客户端不带参数，仍收到原字段集合。未登记的键、重复出现或其他
// 取值都为 400 invalid_query，不静默退回缺省投影。
fn runtime_projection(
    uri: &Uri,
    allow_worker_state: bool,
) -> Result<RuntimeProjection, Box<Response>> {
    let invalid = || {
        let message = if allow_worker_state {
            "runtime 配置读取只接受 include_equipment_taxonomy=true、\
             include=fulfillment_collection_overdue_age_seconds 与 include_worker_restart_state=true"
        } else {
            "runtime 配置只接受 include_equipment_taxonomy=true 与 \
             include=fulfillment_collection_overdue_age_seconds"
        };
        Box::new(api_error(StatusCode::BAD_REQUEST, "invalid_query", message).into_response())
    };
    let allowed: &[&str] = if allow_worker_state {
        &["include_equipment_taxonomy", "include", "include_worker_restart_state"]
    } else {
        &["include_equipment_taxonomy", "include"]
    };
    let query = parse_query(uri, allowed).map_err(|()| invalid())?;
    let worker_restart_state = match query.get("include_worker_restart_state").map(String::as_str) {
        None => false,
        Some("true") => true,
        Some(_) => return Err(invalid()),
    };
    let equipment_taxonomy = match query.get("include_equipment_taxonomy").map(String::as_str) {
        None => false,
        Some("true") => true,
        Some(_) => return Err(invalid()),
    };
    let collection_overdue_threshold = match query.get("include").map(String::as_str) {
        None => false,
        Some(COLLECTION_OVERDUE_AGE_KEY) => true,
        Some(_) => return Err(invalid()),
    };
    Ok(RuntimeProjection { equipment_taxonomy, collection_overdue_threshold, worker_restart_state })
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminRuntime(
            error @ (AdminRuntimeConfigError::InvalidDocument(_)
            | AdminRuntimeConfigError::InvalidCommand(_)),
        ) => {
            // 只匹配固定指引，避免将配置错误的底层细节或路径带进 DTO。
            let hint = if let AdminRuntimeConfigError::InvalidDocument(reason) = error {
                [
                    AdminRuntimeAlertsConfig::WORKER_CONFIG_REQUIRED_HINT,
                    AdminRuntimeAlertsConfig::WORKER_CONFIG_UNREADABLE_HINT,
                    AdminRuntimeAlertsConfig::WORKER_CONFIG_CHANGED_HINT,
                ]
                .into_iter()
                .find(|hint| *hint == reason.as_str())
            } else {
                None
            };
            api_error(
                StatusCode::BAD_REQUEST,
                "invalid_runtime_config",
                hint.unwrap_or("runtime 配置文档无效"),
            )
            .into_response()
        }
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
