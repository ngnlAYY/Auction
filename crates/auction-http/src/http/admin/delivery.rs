//! Admin delivery 列表与人工对账命令的 HTTP 映射。
//!
//! 认证、CSRF、fencing 和幂等结果由应用端口/存储 owner 负责；本模块只解析冻结的路由输入
//! 并投影安全 DTO。

use std::net::SocketAddr;

use auction_application::{
    ApplicationError, DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminListRequest,
    DeliveryAdminSnapshot, RequestHash,
};
use auction_contracts::{
    AdminDeliveryCommandRequest, AdminDeliveryCommandResponse, AdminDeliveryListResponse,
    AdminDeliveryResponse, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use uuid::Uuid;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 delivery Admin 列表。
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
    let query = match super::parse_query(&uri, &["page", "page_size", "status"]) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(auction_application::MAX_DELIVERY_ADMIN_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) => offset,
        None => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request =
        DeliveryAdminListRequest { status: query.get("status").cloned(), limit: page_size, offset };
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let page_result = match port.list(request).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || page_result.total < 0
        || page_result.entries.len() as i64 > page_size
    {
        return super::internal_error();
    }
    let deliveries = page_result.entries.into_iter().map(map_snapshot).collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "交付列表已加载",
            AdminDeliveryListResponse { deliveries, page, page_size, total: page_result.total },
        )),
    )
        .into_response()
}

/// 强制释放仍处于 claimed 的 delivery。
pub(crate) async fn release(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute(state, address, delivery_id, headers, uri, body, DeliveryAdminAction::Release).await
}

/// 把明确失败的 delivery 重新放回 pending。
pub(crate) async fn retry(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute(state, address, delivery_id, headers, uri, body, DeliveryAdminAction::RetryFailed).await
}

/// 对结果未知的 delivery 执行人工决策。
pub(crate) async fn reconcile(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "对账命令不接受查询参数",
        )
        .into_response();
    }
    if let Some(response) = validate_csrf(&headers) {
        return response;
    }
    let input = match serde_json::from_slice::<AdminDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(action_name) = input.action.as_deref() else {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "对账动作无效",
        )
        .into_response();
    };
    let Some(action) = reconcile_action(action_name) else {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "对账动作无效",
        )
        .into_response();
    };
    execute_command(state, headers, delivery_id, action, input).await
}

async fn execute(
    state: AdminApiState,
    address: SocketAddr,
    delivery_id: String,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    action: DeliveryAdminAction,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "交付命令不接受查询参数",
        )
        .into_response();
    }
    if let Some(response) = validate_csrf(&headers) {
        return response;
    }
    let input = match serde_json::from_slice::<AdminDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.action.is_some() {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "交付命令动作不匹配",
        )
        .into_response();
    }
    execute_command(state, headers, delivery_id, action, input).await
}

async fn execute_command(
    state: AdminApiState,
    headers: HeaderMap,
    delivery_id: String,
    action: DeliveryAdminAction,
    input: AdminDeliveryCommandRequest,
) -> Response {
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() || input.command_id.is_nil() || input.expected_fencing_version <= 0 {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "交付命令参数无效",
        )
        .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let actor_id = identity.actor;
    let hash_input = DeliveryHashInput {
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        action: action_name(action),
        external_reference: input.external_reference.clone(),
        failure_code: input.failure_code.clone(),
        actor_id: actor_id.clone(),
    };
    let request_hash =
        match RequestHash::from_request(action.command_kind(), delivery_id, &hash_input) {
            Ok(hash) => hash,
            Err(_) => return super::internal_error(),
        };
    let command = DeliveryAdminCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        action,
        external_reference: input.external_reference,
        failure_code: input.failure_code,
        actor_id,
        request_id: super::request_id(&headers),
        request_hash,
    };
    let result = match port.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "交付状态已更新",
            AdminDeliveryCommandResponse { delivery: map_snapshot(result.snapshot) },
        )),
    )
        .into_response()
}

fn validate_csrf(headers: &HeaderMap) -> Option<Response> {
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    None
}

fn reconcile_action(value: &str) -> Option<DeliveryAdminAction> {
    match value {
        "retry" => Some(DeliveryAdminAction::ReconcileRetry),
        "succeeded" => Some(DeliveryAdminAction::ReconcileSucceeded),
        "failed" => Some(DeliveryAdminAction::ReconcileFailed),
        _ => None,
    }
}

fn action_name(action: DeliveryAdminAction) -> &'static str {
    match action {
        DeliveryAdminAction::Release => "release",
        DeliveryAdminAction::RetryFailed => "retry_failed",
        DeliveryAdminAction::ReconcileRetry => "retry",
        DeliveryAdminAction::ReconcileSucceeded => "succeeded",
        DeliveryAdminAction::ReconcileFailed => "failed",
    }
}

fn map_snapshot(snapshot: DeliveryAdminSnapshot) -> AdminDeliveryResponse {
    AdminDeliveryResponse {
        id: snapshot.id.to_string(),
        origin_kind: snapshot.origin_kind,
        origin_id: snapshot.origin_id,
        recipient: snapshot.recipient,
        cod: snapshot.cod,
        status: snapshot.status,
        lease_expires_at: snapshot.lease_expires_at.map(super::timestamp),
        fencing_version: snapshot.fencing_version,
        attempt_count: snapshot.attempt_count,
        claimed_at: snapshot.claimed_at.map(super::timestamp),
        sending_at: snapshot.sending_at.map(super::timestamp),
        completed_at: snapshot.completed_at.map(super::timestamp),
        external_reference: snapshot.external_reference,
        failure_code: snapshot.failure_code,
        reason_code: snapshot.reason_code,
        take_required: snapshot.take_required,
        returned_mail_id: snapshot.returned_mail_id,
        created_at: super::timestamp(snapshot.created_at),
        updated_at: super::timestamp(snapshot.updated_at),
    }
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "delivery Admin 仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_delivery_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "交付服务暂不可用",
        )
        .into_response(),
    }
}

#[derive(Debug, Clone, Serialize)]
struct DeliveryHashInput {
    delivery_id: Uuid,
    expected_fencing_version: i64,
    action: &'static str,
    external_reference: Option<String>,
    failure_code: Option<String>,
    actor_id: String,
}
