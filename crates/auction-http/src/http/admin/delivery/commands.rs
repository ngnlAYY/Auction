use std::net::SocketAddr;

use auction_application::{DeliveryAdminAction, DeliveryAdminCommand};
use auction_contracts::{
    ADMIN_DELIVERY_RECONCILE_FAILURE_CODES, AdminDeliveryCommandRequest,
    AdminDeliveryCommandResponse, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AdminApiState, api_error, authenticated_admin_mutation, feature_unavailable,
    map_delivery_error, map_snapshot, no_store_headers, request_id,
};

/// release/retry 的旧 v1 请求体只有两个字段，不能因浏览器共用 DTO 而放宽未知键。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SimpleDeliveryCommandRequest {
    command_id: Uuid,
    expected_fencing_version: i64,
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
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "对账命令不接受查询参数")
            .into_response();
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
    execute_command(state, headers, delivery_id, action, input, identity.actor).await
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
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "交付命令不接受查询参数")
            .into_response();
    }
    let input = match serde_json::from_slice::<SimpleDeliveryCommandRequest>(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let input = AdminDeliveryCommandRequest {
        command_id: input.command_id,
        expected_fencing_version: input.expected_fencing_version,
        action: None,
        external_reference: None,
        failure_code: None,
    };
    execute_command(state, headers, delivery_id, action, input, identity.actor).await
}

async fn execute_command(
    state: AdminApiState,
    headers: HeaderMap,
    delivery_id: String,
    action: DeliveryAdminAction,
    input: AdminDeliveryCommandRequest,
    actor_id: String,
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
    let Some((external_reference, failure_code)) =
        normalize_action_fields(action, input.external_reference, input.failure_code)
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    };
    let request_hash = action.request_hash(
        delivery_id,
        input.expected_fencing_version,
        external_reference.as_deref(),
        failure_code.as_deref(),
        &actor_id,
    );
    let command = DeliveryAdminCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        action,
        external_reference,
        failure_code,
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
            delivery_action_message(action),
            AdminDeliveryCommandResponse { delivery: map_snapshot(result.snapshot) },
        )),
    )
        .into_response()
}

fn normalize_action_fields(
    action: DeliveryAdminAction,
    external_reference: Option<String>,
    failure_code: Option<String>,
) -> Option<(Option<String>, Option<String>)> {
    let external_reference = normalize_empty(external_reference);
    let failure_code = normalize_empty(failure_code);
    let valid = match action {
        DeliveryAdminAction::Release
        | DeliveryAdminAction::RetryFailed
        | DeliveryAdminAction::ReconcileRetry => {
            external_reference.is_none() && failure_code.is_none()
        }
        DeliveryAdminAction::ReconcileSucceeded => {
            is_trimmed_non_empty(external_reference.as_deref()) && failure_code.is_none()
        }
        // 新判失败只接受固定原因码，避免无语义自由文本进入投递与履约失败码。
        DeliveryAdminAction::ReconcileFailed => {
            external_reference.is_none()
                && failure_code
                    .as_deref()
                    .is_some_and(|code| ADMIN_DELIVERY_RECONCILE_FAILURE_CODES.contains(&code))
        }
    };

    if valid { Some((external_reference, failure_code)) } else { None }
}

fn normalize_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn is_trimmed_non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value == value.trim())
}

fn delivery_action_message(action: DeliveryAdminAction) -> &'static str {
    match action {
        DeliveryAdminAction::Release => "交付已释放",
        DeliveryAdminAction::RetryFailed => "失败交付已进入重试队列",
        DeliveryAdminAction::ReconcileRetry
        | DeliveryAdminAction::ReconcileSucceeded
        | DeliveryAdminAction::ReconcileFailed => "交付对账已完成",
    }
}

fn reconcile_action(value: &str) -> Option<DeliveryAdminAction> {
    match value {
        "retry" => Some(DeliveryAdminAction::ReconcileRetry),
        "succeeded" => Some(DeliveryAdminAction::ReconcileSucceeded),
        "failed" => Some(DeliveryAdminAction::ReconcileFailed),
        _ => None,
    }
}
