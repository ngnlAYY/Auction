//! Admin 告警台 HTTP 适配器：列表、导航待办计数、确认与静默命令。
//! HTTP 只负责输入闭集、会话/CSRF 与安全投影映射；告警状态、阈值和通知节奏仍由应用层与
//! 存储层拥有，浏览器不能自行推导或改变告警状态。

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::operations::OperationsAlertError;
use auction_application::operations::{
    MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE, MAX_OPERATIONS_ALERT_WEBHOOK_VIEW,
    OperationsAlertAcknowledgeCommand, OperationsAlertConsoleEntry, OperationsAlertConsoleQuery,
    OperationsAlertFilter, OperationsAlertSilenceCommand, OperationsAlertSource,
    OperationsAlertWebhookEntry, parse_alert_id,
};
use auction_contracts::{
    AdminAlertAcknowledgeRequest, AdminAlertCommandResponse, AdminAlertCommandResultResponse,
    AdminAlertEntryResponse, AdminAlertListResponse, AdminAlertSilenceRequest,
    AdminAlertWebhookEntryResponse, AdminAlertWebhookListResponse, AdminPendingCountsResponse,
    ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use uuid::Uuid;

use crate::diagnostics;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取一页活动或历史告警。
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
    let query = match super::parse_query(&uri, &["status", "source", "page", "page_size"]) {
        Ok(query) => query,
        Err(()) => return super::invalid_query("告警查询参数无效"),
    };
    let filter = match query.get("status").map(String::as_str) {
        None => OperationsAlertFilter::Active,
        Some(value) => match OperationsAlertFilter::parse(value) {
            Some(filter) => filter,
            None => return super::invalid_query("告警查询参数无效"),
        },
    };
    let source = match query.get("source") {
        None => None,
        Some(value) => match OperationsAlertSource::parse(value) {
            Some(source) => Some(source),
            None => return super::invalid_query("告警查询参数无效"),
        },
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, Some(i64::MAX / 2)) {
        Ok(page) => page,
        Err(()) => return super::invalid_query("告警查询参数无效"),
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return super::invalid_query("告警查询参数无效"),
    };
    let request = OperationsAlertConsoleQuery { filter, source, page, page_size };
    if request.validate().is_err() {
        return super::invalid_query("告警查询参数无效");
    }
    let Some(console) = state.operations_alert_console() else {
        return super::storage_unavailable("告警台需要已配置的数据库");
    };
    match console.list_alerts(request, Utc::now()).await {
        Ok(page) => Json(ApiResponse::ok(
            "ok",
            AdminAlertListResponse {
                alerts: page.entries.iter().map(map_entry).collect(),
                page: page.page,
                page_size: page.page_size,
                total: page.total,
                active_total: page.active_total,
                resolved_total: page.resolved_total,
            },
        ))
        .into_response(),
        Err(error) => map_error("list_alerts", error),
    }
}

/// 读取最近的 webhook 投递安全投影；只读，不触发重试或状态迁移。
pub(crate) async fn webhooks(
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
    let query = match super::parse_query(&uri, &["limit"]) {
        Ok(query) => query,
        Err(()) => return super::invalid_query("告警查询参数无效"),
    };
    let limit = match super::parse_positive_i64(
        query.get("limit"),
        50,
        Some(MAX_OPERATIONS_ALERT_WEBHOOK_VIEW as i64),
    ) {
        Ok(limit) => limit,
        Err(()) => return super::invalid_query("告警查询参数无效"),
    };
    let Some(console) = state.operations_alert_console() else {
        return super::storage_unavailable("webhook 运营视图需要已配置的数据库");
    };
    match console.list_webhook_deliveries(limit as usize).await {
        Ok(entries) => Json(ApiResponse::ok(
            "ok",
            AdminAlertWebhookListResponse { entries: entries.iter().map(map_webhook).collect() },
        ))
        .into_response(),
        Err(error) => map_error("list_webhook_deliveries", error),
    }
}

/// 读取导航待办计数：人工对账、待确认恢复、活动告警。
pub(crate) async fn pending_counts(
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
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::invalid_query("告警查询参数无效");
    }
    let Some(console) = state.operations_alert_console() else {
        return super::storage_unavailable("待办计数需要已配置的数据库");
    };
    match console.pending_counts().await {
        Ok(counts) => Json(ApiResponse::ok(
            "ok",
            AdminPendingCountsResponse {
                delivery_manual_reconcile: counts.delivery_manual_reconcile,
                recovery_confirmation: counts.recovery_confirmation,
                active_alerts: counts.active_alerts,
            },
        ))
        .into_response(),
        Err(error) => map_error("read_pending_counts", error),
    }
}

/// 确认一条活动告警；只追加操作者事实。
pub(crate) async fn acknowledge(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(alert_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let identity = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::invalid_query("告警查询参数无效");
    }
    let Some(fingerprint) = parse_alert_id(&alert_id) else {
        return invalid_alert();
    };
    let Ok(request) = serde_json::from_slice::<AdminAlertAcknowledgeRequest>(&body) else {
        return invalid_json();
    };
    let Ok(command_id) = Uuid::parse_str(&request.command_id) else {
        return invalid_alert();
    };
    let Ok(command) = OperationsAlertAcknowledgeCommand::new(
        command_id,
        fingerprint,
        identity.actor,
        request.reason,
    ) else {
        return invalid_alert();
    };
    let Some(console) = state.operations_alert_console() else {
        return super::storage_unavailable("告警台需要已配置的数据库");
    };
    match console.acknowledge_alert(command).await {
        Ok(result) => Json(ApiResponse::ok(
            "ok",
            AdminAlertCommandResponse {
                alert: AdminAlertCommandResultResponse {
                    id: alert_id,
                    action: result.action,
                    replayed: result.replayed,
                    silenced_until: result.silenced_until.map(super::timestamp),
                },
            },
        ))
        .into_response(),
        Err(error) => map_error("acknowledge_alert", error),
    }
}

/// 静默一条活动告警到指定时间；只抑制重复通知。
pub(crate) async fn silence(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(alert_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let identity = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::invalid_query("告警查询参数无效");
    }
    let Some(fingerprint) = parse_alert_id(&alert_id) else {
        return invalid_alert();
    };
    let Ok(request) = serde_json::from_slice::<AdminAlertSilenceRequest>(&body) else {
        return invalid_json();
    };
    let Ok(command_id) = Uuid::parse_str(&request.command_id) else {
        return invalid_alert();
    };
    let Ok(command) = OperationsAlertSilenceCommand::new(
        command_id,
        fingerprint,
        request.silence_seconds,
        identity.actor,
        request.reason,
    ) else {
        return invalid_alert();
    };
    let Some(console) = state.operations_alert_console() else {
        return super::storage_unavailable("告警台需要已配置的数据库");
    };
    match console.silence_alert(command).await {
        Ok(result) => Json(ApiResponse::ok(
            "ok",
            AdminAlertCommandResponse {
                alert: AdminAlertCommandResultResponse {
                    id: alert_id,
                    action: result.action,
                    replayed: result.replayed,
                    silenced_until: result.silenced_until.map(super::timestamp),
                },
            },
        ))
        .into_response(),
        Err(error) => map_error("silence_alert", error),
    }
}

fn map_webhook(entry: &OperationsAlertWebhookEntry) -> AdminAlertWebhookEntryResponse {
    AdminAlertWebhookEntryResponse {
        id: entry.delivery_id.to_string(),
        status: entry.status.clone(),
        kind: entry.kind.as_str().to_owned(),
        scope: entry.scope.clone(),
        source: entry.source.as_str().to_owned(),
        attempt_count: entry.attempt_count,
        next_attempt_at: entry.next_attempt_at.map(super::timestamp),
        sending_at: entry.sending_at.map(super::timestamp),
        completed_at: entry.completed_at.map(super::timestamp),
        failure_code: entry.failure_code.clone(),
        created_at: super::timestamp(entry.created_at),
    }
}

fn map_entry(entry: &OperationsAlertConsoleEntry) -> AdminAlertEntryResponse {
    AdminAlertEntryResponse {
        id: entry.alert_id(),
        kind: entry.kind.as_str().to_owned(),
        scope: entry.scope.clone(),
        source: entry.source.as_str().to_owned(),
        severity: entry.severity().as_str().to_owned(),
        active: entry.active,
        count: entry.count,
        age_seconds: entry.age_seconds,
        first_seen_at: super::timestamp(entry.first_seen_at),
        last_seen_at: super::timestamp(entry.last_seen_at),
        resolved_at: entry.resolved_at.map(super::timestamp),
        last_notified_at: entry.last_notified_at.map(super::timestamp),
        next_notify_at: entry.next_notify_at.map(super::timestamp),
        acknowledged_at: entry.acknowledged_at.map(super::timestamp),
        acknowledged_by: entry.acknowledged_by.clone(),
        silenced_until: entry.silenced_until.map(super::timestamp),
        runbook_anchor: entry.runbook_anchor().to_owned(),
    }
}

fn map_error(operation: &'static str, error: ApplicationError) -> Response {
    // 只记录诊断种类，不输出 fingerprint、原因正文或底层 SQL。
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let response = match &error {
        ApplicationError::OperationsAlert(OperationsAlertError::Invalid) => invalid_alert(),
        ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "同一命令 ID 已用于不同请求")
                .into_response()
        }
        ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim) => {
            super::api_error(StatusCode::CONFLICT, "alert_stale", "告警状态已变化，请刷新后重试")
                .into_response()
        }
        _ => super::storage_unavailable("告警台暂不可用"),
    };
    diagnostics::mapped_response("admin_alerts", operation, diagnostic_kind, response)
}

fn invalid_alert() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_alert_command", "告警命令参数无效")
        .into_response()
}

fn invalid_json() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}
