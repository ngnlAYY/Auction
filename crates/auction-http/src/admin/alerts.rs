//! Admin 告警台 HTTP 适配器：列表、导航待办计数、确认与静默命令。
//! HTTP 只负责输入闭集、会话/CSRF 与安全投影映射；告警状态、阈值和通知节奏仍由应用层与
//! 存储层拥有，浏览器不能自行推导或改变告警状态。

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::operations::OperationsAlertError;
use auction_application::operations::{
    MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE, MAX_OPERATIONS_ALERT_WEBHOOK_VIEW,
    OperationsAlertAcknowledgeCommand, OperationsAlertConsoleEntry, OperationsAlertConsolePage,
    OperationsAlertConsoleQuery, OperationsAlertFilter, OperationsAlertSilenceCommand,
    OperationsAlertSource, OperationsAlertWebhookEntry, parse_alert_id,
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
    let query = match parse_query(&uri, &["status", "source", "page", "page_size", "include"]) {
        Ok(query) => query,
        Err(()) => return invalid_query("告警查询参数无效"),
    };
    let include_incident_generation = match query.get("include").map(String::as_str) {
        None => false,
        Some("incident_generation") => true,
        Some(_) => return invalid_query("附加字段无效"),
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
        Ok(page) => list_response(page, include_incident_generation),
        Err(error) => map_error("list_alerts", error),
    }
}

fn list_response(page: OperationsAlertConsolePage, include_incident_generation: bool) -> Response {
    Json(ApiResponse::ok(
        "ok",
        AdminAlertListResponse {
            alerts: page
                .entries
                .iter()
                .map(|entry| map_entry(entry, include_incident_generation))
                .collect(),
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            active_total: page.active_total,
            resolved_total: page.resolved_total,
        },
    ))
    .into_response()
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
    let command = match acknowledge_command(&body, fingerprint, identity.actor) {
        Ok(command) => command,
        Err(response) => return *response,
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
    let command = match silence_command(&body, fingerprint, identity.actor) {
        Ok(command) => command,
        Err(response) => return *response,
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

fn acknowledge_command(
    body: &[u8],
    fingerprint: [u8; 32],
    actor: String,
) -> Result<OperationsAlertAcknowledgeCommand, Box<Response>> {
    let Ok(request) = serde_json::from_slice::<AdminAlertAcknowledgeRequest>(body) else {
        return Err(Box::new(invalid_json()));
    };
    let Ok(command_id) = Uuid::parse_str(&request.command_id) else {
        return Err(Box::new(invalid_alert()));
    };
    let Ok(command) = OperationsAlertAcknowledgeCommand::new(
        command_id,
        fingerprint,
        request.incident_generation,
        actor,
        request.reason,
    ) else {
        return Err(Box::new(invalid_alert()));
    };
    Ok(command)
}

fn silence_command(
    body: &[u8],
    fingerprint: [u8; 32],
    actor: String,
) -> Result<OperationsAlertSilenceCommand, Box<Response>> {
    let Ok(request) = serde_json::from_slice::<AdminAlertSilenceRequest>(body) else {
        return Err(Box::new(invalid_json()));
    };
    let Ok(command_id) = Uuid::parse_str(&request.command_id) else {
        return Err(Box::new(invalid_alert()));
    };
    let Ok(command) = OperationsAlertSilenceCommand::new(
        command_id,
        fingerprint,
        request.incident_generation,
        request.silence_seconds,
        actor,
        request.reason,
    ) else {
        return Err(Box::new(invalid_alert()));
    };
    Ok(command)
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

fn map_entry(
    entry: &OperationsAlertConsoleEntry,
    include_incident_generation: bool,
) -> AdminAlertEntryResponse {
    AdminAlertEntryResponse {
        id: entry.alert_id(),
        kind: entry.kind.as_str().to_owned(),
        scope: entry.scope.clone(),
        source: entry.source.as_str().to_owned(),
        severity: entry.severity().as_str().to_owned(),
        active: entry.active,
        incident_generation: include_incident_generation.then_some(entry.incident_generation),
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
    use std::{borrow::Cow, future::ready, sync::Arc};

    use auction_application::core::ApplicationFuture;
    use auction_application::operations::{
        AdminPendingCounts, MAX_OPERATIONS_ALERT_SILENCE_SECONDS,
        OperationsAlertAcknowledgeCommand, OperationsAlertCommandResult, OperationsAlertConsole,
        OperationsAlertConsoleEntry, OperationsAlertConsolePage, OperationsAlertConsoleQuery,
        OperationsAlertKind, OperationsAlertSilenceCommand, OperationsAlertSource,
        OperationsAlertWebhookEntry,
    };
    use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
    use auction_contracts::{AdminAlertAcknowledgeRequest, AdminAlertSilenceRequest};
    use auction_time::{Duration, Timestamp};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request, StatusCode, Uri, header};
    use axum::response::Response;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{acknowledge_command, pending_counts_projection, silence_command};
    use crate::{AdminApiState, AdminHttpSecurity, admin_router};

    struct NoAssets;

    impl rust_embed::RustEmbed for NoAssets {
        fn get(_path: &str) -> Option<rust_embed::EmbeddedFile> {
            None
        }

        fn iter() -> impl Iterator<Item = Cow<'static, str>> {
            std::iter::empty()
        }
    }

    struct AlertTestPorts;

    impl AdminSessionPort for AlertTestPorts {
        fn login(
            &self,
            _actor: String,
            _secret: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminLoginResult> {
            panic!("告警路由测试使用既有会话")
        }

        fn current(
            &self,
            raw_token: String,
            raw_csrf_token: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            assert_eq!((raw_token.as_str(), raw_csrf_token.as_str()), ("fixture", "fixture"));
            Box::pin(ready(Ok(AdminSessionIdentity {
                actor: "operator".to_owned(),
                expires_at: now + Duration::minutes(15),
                absolute_expires_at: now + Duration::hours(8),
            })))
        }

        fn touch(
            &self,
            raw_token: String,
            raw_csrf_token: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            self.current(raw_token, raw_csrf_token, now)
        }

        fn logout(&self, _raw_token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            panic!("告警路由测试无需退出会话")
        }
    }

    impl OperationsAlertConsole for AlertTestPorts {
        fn list_alerts(
            &self,
            query: OperationsAlertConsoleQuery,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, OperationsAlertConsolePage> {
            let page = test_alert_page();
            page.validate(&query).expect("列表夹具匹配请求");
            Box::pin(ready(Ok(page)))
        }

        fn acknowledge_alert(
            &self,
            command: OperationsAlertAcknowledgeCommand,
        ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
            assert_eq!(command.reason, worst_case_reason());
            Box::pin(ready(Ok(OperationsAlertCommandResult {
                command_id: command.command_id,
                fingerprint: command.fingerprint,
                action: "acknowledge".to_owned(),
                silenced_until: None,
                replayed: false,
            })))
        }

        fn silence_alert(
            &self,
            command: OperationsAlertSilenceCommand,
        ) -> ApplicationFuture<'_, OperationsAlertCommandResult> {
            assert_eq!(command.reason, worst_case_reason());
            Box::pin(ready(Ok(OperationsAlertCommandResult {
                command_id: command.command_id,
                fingerprint: command.fingerprint,
                action: "silence".to_owned(),
                silenced_until: Some(Timestamp::now() + Duration::seconds(command.silence_seconds)),
                replayed: false,
            })))
        }

        fn pending_counts(&self) -> ApplicationFuture<'_, AdminPendingCounts> {
            panic!("告警路由测试无需读取待办计数")
        }

        fn list_webhook_deliveries(
            &self,
            _limit: usize,
        ) -> ApplicationFuture<'_, Vec<OperationsAlertWebhookEntry>> {
            panic!("告警路由测试无需读取 webhook 投递")
        }
    }

    fn test_router() -> Router {
        let state = AdminApiState::new(
            Some(Arc::new(AlertTestPorts)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            AdminHttpSecurity::default(),
        )
        .with_operations_alert_console(Some(Arc::new(AlertTestPorts)));
        admin_router::<NoAssets>("test", state.http_boundary()).with_state(state)
    }

    async fn request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        "auction_admin_session=fixture; auction_admin_csrf=fixture",
                    )
                    .header("x-csrf-token", "fixture")
                    .body(Body::from(body))
                    .expect("构造告警 HTTP 请求"),
            )
            .await
            .expect("调用告警路由");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取告警 HTTP 响应");
        (status, serde_json::from_slice(&body).expect("告警响应是 JSON"))
    }

    async fn alert_command_error(response: Response, expected_code: &str) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取告警命令错误");
        let body: Value = serde_json::from_slice(&body).expect("告警命令错误是 JSON");
        assert_eq!(body["code"], expected_code);
    }

    #[tokio::test]
    async fn alert_commands_require_nonnegative_integer_generations() {
        for silence in [false, true] {
            let parse = |generation| {
                let mut body = json!({"command_id": "01234567-89ab-4cde-8f01-23456789abcd"});
                if let Some(generation) = generation {
                    body["incident_generation"] = generation;
                }
                let actor = "admin".to_owned();
                if silence {
                    body["silence_seconds"] = json!(3_600);
                    silence_command(body.to_string().as_bytes(), [1; 32], actor)
                        .map(|command| command.incident_generation)
                } else {
                    acknowledge_command(body.to_string().as_bytes(), [1; 32], actor)
                        .map(|command| command.incident_generation)
                }
            };
            let missing = parse(None).expect_err("缺失的事件代数不能视作任意一轮");
            alert_command_error(*missing, "invalid_json").await;
            for malformed in [json!(1.5), json!("1"), Value::Null] {
                let response = parse(Some(malformed)).expect_err("事件代数必须是整数");
                alert_command_error(*response, "invalid_json").await;
            }
            let negative = parse(Some(json!(-1))).expect_err("事件代数不能为负数");
            alert_command_error(*negative, "invalid_alert_command").await;
            assert_eq!(parse(Some(json!(0))).expect("接受零代数"), 0);
            assert_eq!(parse(Some(json!(2))).expect("接受当前代数"), 2);
        }
    }

    fn test_alert_page() -> OperationsAlertConsolePage {
        let seen = Timestamp::from_ymd_hms(2026, 11, 18, 12, 0, 0).expect("观察时间");
        let first = OperationsAlertConsoleEntry {
            fingerprint: [0xab; 32],
            kind: OperationsAlertKind::ServiceMissing,
            scope: "worker".to_owned(),
            source: OperationsAlertSource::Process,
            active: true,
            incident_generation: 2,
            count: 1,
            age_seconds: 0,
            first_seen_at: seen,
            last_seen_at: seen,
            resolved_at: None,
            last_notified_at: None,
            next_notify_at: Some(seen + Duration::minutes(2)),
            acknowledged_at: None,
            acknowledged_by: None,
            silenced_until: None,
        };
        let second = OperationsAlertConsoleEntry {
            fingerprint: [0xcd; 32],
            scope: "admin".to_owned(),
            incident_generation: 1,
            age_seconds: 60,
            first_seen_at: seen - Duration::minutes(2),
            last_seen_at: seen - Duration::minutes(1),
            last_notified_at: Some(seen - Duration::minutes(2)),
            next_notify_at: Some(seen + Duration::hours(1)),
            acknowledged_at: Some(seen),
            acknowledged_by: Some("operator".to_owned()),
            silenced_until: Some(seen + Duration::hours(1)),
            ..first
        };
        OperationsAlertConsolePage {
            entries: vec![first, second],
            page: 1,
            page_size: 50,
            total: 2,
            active_total: 2,
            resolved_total: 0,
        }
    }

    #[tokio::test]
    async fn alert_list_serializes_the_selected_projection() {
        let app = test_router();
        let mut expected: Value = serde_json::from_str(include_str!(
            "../../../auction-contracts/tests/fixtures/admin/alerts-list.json"
        ))
        .expect("告警列表样本");
        let (status, body) = request(
            &app,
            Method::GET,
            "/api/v1/admin/alerts?status=active&source=process&page=1&page_size=50&include=incident_generation",
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, expected);

        for entry in expected["data"]["alerts"].as_array_mut().expect("告警条目数组") {
            entry
                .as_object_mut()
                .expect("告警条目")
                .remove("incident_generation")
                .expect("样本包含代数");
        }
        let (status, body) = request(&app, Method::GET, "/api/v1/admin/alerts", vec![]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, expected, "缺省投影应保留旧字段闭集");
    }

    #[tokio::test]
    async fn alert_list_rejects_invalid_include() {
        let app = test_router();
        for (query, message) in [
            ("include=", "附加字段无效"),
            ("include=INCIDENT_GENERATION", "附加字段无效"),
            ("include=settlement_returns", "附加字段无效"),
            ("include=incident_generation,settlement_returns", "附加字段无效"),
            ("include=incident_generation&include=incident_generation", "告警查询参数无效"),
            ("fields=summary", "告警查询参数无效"),
        ] {
            let uri = format!("/api/v1/admin/alerts?{query}");
            let (status, body) = request(&app, Method::GET, &uri, vec![]).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(body["code"], "invalid_query", "{uri}");
            assert_eq!(body["message"], message, "{uri}");
        }
    }

    fn worst_case_reason() -> String {
        "a".repeat(2_048)
    }

    #[tokio::test]
    async fn alert_routes_accept_the_worst_case_serialized_note_within_the_body_limit() {
        // 合法 ASCII 每个字节可以用 6 字节的 \uXXXX 表示。静默请求的其余字段共 141 字节
        // （含 45 字节的 URN UUID）。2048 × 6 + 141 = 12429，向上取整为 13 KiB。
        const BODY_LIMIT: usize = 13 << 10;
        let command_id = "urn:uuid:01234567-89ab-4cde-8f01-23456789abcd".to_owned();
        let reason = worst_case_reason();
        let acknowledge = AdminAlertAcknowledgeRequest {
            command_id: command_id.clone(),
            incident_generation: i64::MAX,
            reason: reason.clone(),
        };
        let silence = AdminAlertSilenceRequest {
            command_id,
            incident_generation: i64::MAX,
            reason: reason.clone(),
            silence_seconds: MAX_OPERATIONS_ALERT_SILENCE_SECONDS,
        };
        let escaped_reason = r"\u0061".repeat(reason.len());
        let oversized_reason = format!(r"{escaped_reason}\u0061");
        let app = test_router();
        for (action, serialized, expected_size) in [
            ("acknowledge", serde_json::to_string(&acknowledge).expect("序列化确认请求"), 12_404),
            ("silence", serde_json::to_string(&silence).expect("序列化静默请求"), 12_429),
        ] {
            let uri = format!("/api/v1/admin/alerts/{}/{action}", "ab".repeat(32));
            // 保留 DTO 序列化的外壳，仅把备注换成等价的六字节转义。
            let mut body = serialized.replace(&reason, &escaped_reason).into_bytes();
            assert_eq!(body.len(), expected_size);
            let (status, response) = request(&app, Method::POST, &uri, body.clone()).await;
            assert_eq!(status, StatusCode::OK, "{action}: {response}");
            assert_eq!(response["data"]["alert"]["action"], action);

            let invalid_body = serialized.replace(&reason, &oversized_reason).into_bytes();
            assert!(invalid_body.len() <= BODY_LIMIT);
            let (status, response) = request(&app, Method::POST, &uri, invalid_body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{action}: {response}");
            assert_eq!(response["code"], "invalid_alert_command");

            // 在合法 JSON 末尾补空白，只改变线上长度，验证门禁恰在 13 KiB 后拒绝。
            body.resize(BODY_LIMIT, b' ');
            let (status, response) = request(&app, Method::POST, &uri, body.clone()).await;
            assert_eq!(status, StatusCode::OK, "{action}: {response}");
            body.push(b' ');
            let (status, response) = request(&app, Method::POST, &uri, body).await;
            assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{action}: {response}");
            assert_eq!(response["code"], "body_too_large");
        }
    }

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
