use std::net::SocketAddr;

use auction_application::{WorkerControlCondition, WorkerControlTrigger};
use auction_contracts::{AdminMailPollTriggerRequest, AdminMailPollTriggerResponse, ApiResponse};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    ADMIN_CSRF_COOKIE, AdminApiState, admin_session_guard, authenticated_admin_session,
    authentication_error, feature_unavailable, no_store_headers, read_admin_cookie,
    valid_admin_csrf_header,
};

/// 通过 Worker control 一次唤醒全部已装配邮箱的轮询器。
pub(crate) async fn trigger_poll(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
            .into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "邮件轮询触发不接受查询参数",
        )
        .into_response();
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let input: AdminMailPollTriggerRequest = match serde_json::from_slice(&body) {
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
    if state.admin_mail().is_none() {
        return feature_unavailable();
    }
    let Some(worker_control) = state.worker_control() else {
        return worker_unavailable();
    };
    let result = match worker_control.trigger_poll(input.command_id).await {
        Ok(result) => result,
        Err(_) => return worker_unavailable(),
    };
    if result.condition != WorkerControlCondition::Available
        || !result.accepted
        || result.pollers <= 0
    {
        return map_worker_trigger(result);
    }
    (
        StatusCode::ACCEPTED,
        no_store_headers(),
        Json(ApiResponse::accepted(
            "邮件轮询已触发",
            AdminMailPollTriggerResponse {
                command_id: input.command_id.to_string(),
                pollers: result.pollers,
            },
        )),
    )
        .into_response()
}

fn worker_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        .into_response()
}

fn map_worker_trigger(result: WorkerControlTrigger) -> Response {
    match result.condition {
        WorkerControlCondition::Timeout => {
            super::api_error(StatusCode::GATEWAY_TIMEOUT, "timeout", "请求处理超时").into_response()
        }
        WorkerControlCondition::Rejected => {
            super::api_error(StatusCode::CONFLICT, "poll_rejected", "Worker 当前无法触发邮件轮询")
                .into_response()
        }
        WorkerControlCondition::Unavailable
        | WorkerControlCondition::Incompatible
        | WorkerControlCondition::Available => worker_unavailable(),
    }
}
