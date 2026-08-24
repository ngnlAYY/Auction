use std::net::SocketAddr;

use auction_application::{DeliveryAdminAction, DeliveryAdminCommand, RequestHash};
use auction_contracts::{AdminDeliveryCommandRequest, AdminDeliveryCommandResponse, ApiResponse};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use uuid::Uuid;

use super::{
    ADMIN_CSRF_COOKIE, AdminApiState, admin_session_guard, api_error, authenticated_admin_session,
    authentication_error, feature_unavailable, internal_error, map_delivery_error, map_snapshot,
    no_store_headers, read_admin_cookie, request_id, valid_admin_csrf_header,
};

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
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "对账命令不接受查询参数")
            .into_response();
    }
    if let Some(response) = validate_csrf(&headers) {
        return response;
    }
    let input = match serde_json::from_slice::<AdminDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(action_name) = input.action.as_deref() else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "对账动作无效")
            .into_response();
    };
    let Some(action) = reconcile_action(action_name) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "对账动作无效")
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
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "交付命令不接受查询参数")
            .into_response();
    }
    if let Some(response) = validate_csrf(&headers) {
        return response;
    }
    let input = match serde_json::from_slice::<AdminDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.action.is_some() {
        return api_error(
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
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() || input.command_id.is_nil() || input.expected_fencing_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
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
            Err(_) => return internal_error(),
        };
    let command = DeliveryAdminCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        action,
        external_reference: input.external_reference,
        failure_code: input.failure_code,
        actor_id,
        request_id: request_id(&headers),
        request_hash,
    };
    let result = match port.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "交付状态已更新",
            AdminDeliveryCommandResponse { delivery: map_snapshot(result.snapshot) },
        )),
    )
        .into_response()
}

fn validate_csrf(headers: &HeaderMap) -> Option<Response> {
    let Some(raw_csrf) = read_admin_cookie(headers, ADMIN_CSRF_COOKIE) else {
        return Some(authentication_error());
    };
    if !valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response(),
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

#[derive(Debug, Clone, Serialize)]
struct DeliveryHashInput {
    delivery_id: Uuid,
    expected_fencing_version: i64,
    action: &'static str,
    external_reference: Option<String>,
    failure_code: Option<String>,
    actor_id: String,
}
