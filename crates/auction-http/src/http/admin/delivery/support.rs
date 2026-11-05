//! W4.6 玩家支持只读汇总的 HTTP 适配器。
//!
//! 只接受 `user`（用户名或用户 UUID）与 `reference`（规范 UUID）之一，二者都给或都给空都
//! 失败关闭；不提供写动作，也不把投递 payload、正文或凭据带出安全投影。

use std::net::SocketAddr;

use auction_application::{PlayerSupportRequest, PlayerSupportSelector};
use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_delivery_error, map_player_support, no_store_headers,
    parse_optional_filter, parse_optional_reference, parse_query,
};

/// 读取玩家支持汇总；响应固定不分页，超过分区上限时由 `truncated` 提示。
pub(crate) async fn player_support(
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
    let query = match parse_query(&uri, &["user", "reference"]) {
        Ok(query) => query,
        Err(()) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let user = match parse_optional_filter(
        &query,
        "user",
        auction_application::MAX_PLAYER_SUPPORT_USER_LEN,
    ) {
        Ok(user) => user,
        Err(()) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let reference = match parse_optional_reference(&query, "reference") {
        Ok(reference) => reference,
        Err(()) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    // 两个入口互斥：一次查询只回答一种身份/参考，避免把无关记录混进同一份汇总。
    let selector = match (user, reference) {
        (Some(user), None) => PlayerSupportSelector::User(user),
        (None, Some(reference)) => PlayerSupportSelector::Reference(reference),
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = PlayerSupportRequest { selector };
    if !request.validate() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let bundle = match port.player_support(request).await {
        Ok(bundle) => bundle,
        Err(error) => return map_delivery_error(error),
    };
    let limit = auction_application::MAX_PLAYER_SUPPORT_ROWS;
    let count = |length: usize| i64::try_from(length).ok();
    let within_limit = [
        count(bundle.bids.len()),
        count(bundle.deals.len()),
        count(bundle.obligations.len()),
        count(bundle.deliveries.len()),
        count(bundle.notifications.len()),
    ]
    .into_iter()
    .all(|length| length.is_some_and(|length| length >= 0 && length <= limit));
    if !within_limit {
        return internal_error();
    }
    let data = map_player_support(bundle);
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("玩家支持汇总已加载", data)))
        .into_response()
}
