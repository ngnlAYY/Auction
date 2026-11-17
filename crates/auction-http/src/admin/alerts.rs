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
use auction_time::Timestamp;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{api_error, invalid_query, storage_unavailable};
use crate::mapping::timestamp;
use crate::query::{parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取一页活动或历史告警。
pub(crate) async fn list(
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
    let query = match parse_query(&uri, &["status", "source", "page", "page_size"]) {
        Ok(query) => query,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let filter = match query.get("status").map(String::as_str) {
        None => OperationsAlertFilter::Active,
        Some(value) => match OperationsAlertFilter::parse(value) {
            Some(filter) => filter,
            None => return invalid_query("告警查询参数无效"),
        },
    };
    let source = match query.get("source") {
        None => None,
        Some(value) => match OperationsAlertSource::parse(value) {
            Some(source) => Some(source),
            None => return invalid_query("告警查询参数无效"),
        },
    };
    let page = match parse_positive_i64(query.get("page"), 1, Some(i64::MAX / 2)) {
        Ok(page) => page,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let request = OperationsAlertConsoleQuery { filter, source, page, page_size };
    if request.validate().is_err() {
        return invalid_query("告警查询参数无效");
    }
    let Some(console) = state.operations_alert_console() else {
        return storage_unavailable("告警台需要已配置的数据库");
    };
    match console.list_alerts(request, Timestamp::now()).await {
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
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match parse_query(&uri, &["limit"]) {
        Ok(query) => query,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let limit = match parse_positive_i64(
        query.get("limit"),
        50,
        Some(MAX_OPERATIONS_ALERT_WEBHOOK_VIEW as i64),
    ) {
        Ok(limit) => limit,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let Some(console) = state.operations_alert_console() else {
        return storage_unavailable("webhook 运营视图需要已配置的数据库");
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

/// 读取导航待办计数：人工对账、待确认恢复、活动告警；带 `include=settlement_returns` 时追加
/// 未结案结算退件数。
pub(crate) async fn pending_counts(
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
    let include_returns = match pending_counts_projection(&uri) {
        Ok(include_returns) => include_returns,
        Err(response) => return *response,
    };
    let Some(console) = state.operations_alert_console() else {
        return storage_unavailable("待办计数需要已配置的数据库");
    };
    match console.pending_counts().await {
        Ok(counts) => Json(ApiResponse::ok(
            "ok",
            AdminPendingCountsResponse {
                delivery_manual_reconcile: counts.delivery_manual_reconcile,
                recovery_confirmation: counts.recovery_confirmation,
                active_alerts: counts.active_alerts,
                settlement_returns: include_returns.then_some(counts.settlement_returns),
            },
        ))
        .into_response(),
        Err(error) => map_error("read_pending_counts", error),
    }
}

/// 待办计数的响应投影：缺省是已发布的三字段闭集；只接受可选的 `include=settlement_returns`
/// 追加退件计数。未登记的键或重复出现为“告警查询参数无效”，`include` 的其他取值（含空值、
/// 大小写不同与逗号列表）为“附加字段无效”，都不静默退回缺省投影。
fn pending_counts_projection(uri: &Uri) -> Result<bool, Box<Response>> {
    let mut query =
        parse_query(uri, &["include"]).map_err(|()| Box::new(invalid_query("告警查询参数无效")))?;
    match query.remove("include").as_deref() {
        None => Ok(false),
        Some("settlement_returns") => Ok(true),
        Some(_) => Err(Box::new(invalid_query("附加字段无效"))),
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
    let identity = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return invalid_query("告警查询参数无效");
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
        return storage_unavailable("告警台需要已配置的数据库");
    };
    match console.acknowledge_alert(command).await {
        Ok(result) => Json(ApiResponse::ok(
            "ok",
            AdminAlertCommandResponse {
                alert: AdminAlertCommandResultResponse {
                    id: alert_id,
                    action: result.action,
                    replayed: result.replayed,
                    silenced_until: result.silenced_until.map(timestamp),
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
    let identity = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok((_, identity)) => identity,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return invalid_query("告警查询参数无效");
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
        return storage_unavailable("告警台需要已配置的数据库");
    };
    match console.silence_alert(command).await {
        Ok(result) => Json(ApiResponse::ok(
            "ok",
            AdminAlertCommandResponse {
                alert: AdminAlertCommandResultResponse {
                    id: alert_id,
                    action: result.action,
                    replayed: result.replayed,
                    silenced_until: result.silenced_until.map(timestamp),
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
        next_attempt_at: entry.next_attempt_at.map(timestamp),
        sending_at: entry.sending_at.map(timestamp),
        completed_at: entry.completed_at.map(timestamp),
        failure_code: entry.failure_code.clone(),
        created_at: timestamp(entry.created_at),
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
        first_seen_at: timestamp(entry.first_seen_at),
        last_seen_at: timestamp(entry.last_seen_at),
        resolved_at: entry.resolved_at.map(timestamp),
        last_notified_at: entry.last_notified_at.map(timestamp),
        next_notify_at: entry.next_notify_at.map(timestamp),
        acknowledged_at: entry.acknowledged_at.map(timestamp),
        acknowledged_by: entry.acknowledged_by.clone(),
        silenced_until: entry.silenced_until.map(timestamp),
        runbook_anchor: entry.runbook_anchor().to_owned(),
    }
}

fn map_error(operation: &'static str, error: ApplicationError) -> Response {
    // 只记录诊断种类，不输出 fingerprint、原因正文或底层 SQL。
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let response = match &error {
        ApplicationError::OperationsAlert(OperationsAlertError::Invalid) => invalid_alert(),
        ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "同一命令 ID 已用于不同请求")
                .into_response()
        }
        ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim) => {
            api_error(StatusCode::CONFLICT, "alert_stale", "告警状态已变化，请刷新后重试")
                .into_response()
        }
        _ => storage_unavailable("告警台暂不可用"),
    };
    diagnostics::mapped_response("admin_alerts", operation, diagnostic_kind, response)
}

fn invalid_alert() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_alert_command", "告警命令参数无效").into_response()
}

fn invalid_json() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{StatusCode, Uri};
    use serde_json::Value;

    use super::pending_counts_projection;

    async fn parse(query: &str) -> Result<bool, (StatusCode, Value)> {
        let uri: Uri = format!("/api/v1/admin/pending-counts{query}").parse().expect("测试 URI");
        match pending_counts_projection(&uri) {
            Ok(include) => Ok(include),
            Err(response) => {
                let status = response.status();
                let bytes =
                    to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
                Err((status, serde_json::from_slice(&bytes).expect("错误响应是 JSON")))
            }
        }
    }

    /// 缺省响应保持已发布的三字段闭集；只有显式 `include=settlement_returns` 追加退件计数。
    #[tokio::test]
    async fn settlement_returns_count_is_opt_in() {
        assert_eq!(parse("").await, Ok(false));
        assert_eq!(parse("?").await, Ok(false));
        assert_eq!(parse("?include=settlement_returns").await, Ok(true));
    }

    /// 未知取值、空值、大小写不同与逗号列表为“附加字段无效”；重复出现或未登记的键为“告警查询
    /// 参数无效”，都不静默退回缺省投影。
    #[tokio::test]
    async fn unknown_or_repeated_include_fails_closed() {
        for (query, message) in [
            ("?include=", "附加字段无效"),
            ("?include=SETTLEMENT_RETURNS", "附加字段无效"),
            ("?include=settlement_returns,alerts", "附加字段无效"),
            ("?include=settlement_returns&include=settlement_returns", "告警查询参数无效"),
            ("?status=active", "告警查询参数无效"),
        ] {
            let (status, body) = parse(query).await.expect_err(query);
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }
}
