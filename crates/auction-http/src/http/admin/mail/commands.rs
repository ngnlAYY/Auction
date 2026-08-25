use std::net::SocketAddr;

use super::{
    ADMIN_CSRF_COOKIE, AdminApiState, admin_session_guard, authenticated_admin_session,
    authentication_error, feature_unavailable, internal_error, map_error, no_store_headers,
    read_admin_cookie, request_id, valid_admin_csrf_header,
};
use auction_application::{AdminMailPollingCommand, RequestHash, admin_mail_aggregate_id};
use auction_contracts::{AdminMailPollingRequest, AdminMailStateResponse, ApiResponse};
use auction_domain::CommandId;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

/// 以版本 fence 暂停或恢复指定邮箱轮询。
pub(crate) async fn set_paused(
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
            "邮件轮询命令不接受查询参数",
        )
        .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminMailPollingRequest = match serde_json::from_slice(&body) {
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
    let aggregate_id = admin_mail_aggregate_id(&input.mailbox_key);
    let request_hash =
        match RequestHash::from_request("admin_mail_set_polling_paused", aggregate_id, &input) {
            Ok(hash) => hash,
            Err(_) => return internal_error(),
        };
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port
        .set_polling_paused(AdminMailPollingCommand {
            command_id: CommandId::from_uuid(input.command_id),
            mailbox_key: input.mailbox_key.clone(),
            paused: input.paused,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let Some(expected_state_version) = input.expected_state_version.checked_add(1) else {
        return internal_error();
    };
    if !result.validate()
        || result.mailbox_key != input.mailbox_key
        || result.paused != input.paused
        || result.state_version != expected_state_version
    {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "邮件轮询状态已更新",
            AdminMailStateResponse { polling: super::state::map_poll_state(result) },
        )),
    )
        .into_response()
}
