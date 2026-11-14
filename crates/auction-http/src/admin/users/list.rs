//! Admin 用户列表的处理器。

use std::net::SocketAddr;

use auction_application::admin::{AdminUserListRequest, AdminUserStatus};
use auction_contracts::{AdminUserListResponse, AdminUserPaginationResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, boundary, map_list_user,
    parse_positive_i64, parse_query,
};

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 Admin 用户列表。
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
    let mut query = match parse_query(&uri, &["name", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return boundary::invalid_query(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return boundary::invalid_query(),
    };
    let page_size = match parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
        Ok(value) => value,
        Err(_) => return boundary::invalid_query(),
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(value) if value <= auction_application::admin::MAX_ADMIN_USER_OFFSET => value,
        _ => return boundary::invalid_query(),
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminUserStatus::parse(value) {
            Some(status) => Some(status),
            None => return boundary::invalid_query(),
        },
    };
    let request =
        AdminUserListRequest { name: query.remove("name"), status, limit: page_size, offset };
    let Some(port) = state.admin_users() else {
        return boundary::feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return boundary::map_list_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }
    let response = AdminUserListResponse {
        users: result.users.into_iter().map(map_list_user).collect(),
        pagination: AdminUserPaginationResponse { page, page_size, total: result.total },
    };
    (
        StatusCode::OK,
        super::super::no_store_headers(),
        Json(ApiResponse::ok("用户列表已加载", response)),
    )
        .into_response()
}
