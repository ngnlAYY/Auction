//! Admin 邮箱轮询状态的处理器。

use std::net::SocketAddr;

use auction_application::admin::{AdminMailPollState, AdminMailPollStateRequest};
use auction_application::core::ApplicationError;
use auction_contracts::{AdminMailPollingResponse, AdminMailStateResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_error, no_store_headers, parse_query, timestamp,
};

/// 读取指定邮箱的轮询状态。
pub(crate) async fn state(
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
    let query = match parse_query(&uri, &["mailbox_key", "include_diagnostics"]) {
        Ok(query) if query.get("mailbox_key").is_some_and(|value| !value.is_empty()) => query,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let include_diagnostics = match query.get("include_diagnostics").map(String::as_str) {
        None => false,
        Some("true") => true,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = AdminMailPollStateRequest {
        mailbox_key: query.get("mailbox_key").cloned().unwrap_or_default(),
    };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.poll_state(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate() {
        return internal_error();
    }
    let diagnostics = if include_diagnostics {
        Some(super::diagnostics::read_polling_diagnostics(&state, &result).await)
    } else {
        None
    };
    let mut polling = map_poll_state(result);
    polling.diagnostics = diagnostics;
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("邮件轮询状态已加载", AdminMailStateResponse { polling })),
    )
        .into_response()
}

/// 将应用层轮询状态映射为安全 HTTP 投影。
pub(crate) fn map_poll_state(state: AdminMailPollState) -> AdminMailPollingResponse {
    AdminMailPollingResponse {
        mailbox_key: state.mailbox_key,
        last_poll_at: timestamp(state.last_poll_at),
        next_poll_at: timestamp(state.next_poll_at),
        consecutive_failures: state.consecutive_failures,
        paused: state.paused,
        state_version: state.state_version,
        updated_at: timestamp(state.updated_at),
        diagnostics: None,
    }
}
