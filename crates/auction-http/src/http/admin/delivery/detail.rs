//! Admin 投递详情读取：聚合尝试、Sent 证据、领取、处置与来源义务的安全投影。

use std::net::SocketAddr;

use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_session,
    feature_unavailable, map_delivery_error, map_detail, no_store_headers,
};

/// 读取单条 delivery 的完整证据详情；不返回 payload、正文或凭据秘密。
pub(crate) async fn detail(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Ok(delivery_id) = Uuid::parse_str(&delivery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    };
    if delivery_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let detail = match port.detail(delivery_id).await {
        Ok(detail) => detail,
        Err(error) => return map_delivery_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("交付详情已加载", map_detail(detail))),
    )
        .into_response()
}
