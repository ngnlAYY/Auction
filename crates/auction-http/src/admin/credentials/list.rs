//! Admin 凭据列表的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminCredentialKind, AdminCredentialListRequest, AdminCredentialStatus, AdminCredentialWorld,
};
use auction_contracts::{
    AdminCredentialListResponse, AdminCredentialPaginationResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{feature_unavailable, invalid_query, map_credential, map_error};
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取凭据安全投影列表。
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
    let query = match parse_query(&uri, &["kind", "world", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return invalid_query(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page_size = match parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let Some(offset) =
        page_offset(page, page_size, Some(auction_application::admin::MAX_ADMIN_CREDENTIAL_OFFSET))
    else {
        return invalid_query();
    };
    let kind = match query.get("kind").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialKind::parse(value) {
            Some(kind) => Some(kind),
            None => return invalid_query(),
        },
    };
    let world = match query.get("world").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialWorld::parse(value) {
            Some(world) => Some(world),
            None => return invalid_query(),
        },
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialStatus::parse(value) {
            Some(status) => Some(status),
            None => return invalid_query(),
        },
    };
    let request = AdminCredentialListRequest { kind, world, status, limit: page_size, offset };
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return internal_error();
    }
    let response = AdminCredentialListResponse {
        credentials: result.credentials.into_iter().map(map_credential).collect(),
        pagination: AdminCredentialPaginationResponse { page, page_size, total: result.total },
        restart_required: true,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("凭据列表已加载", response)))
        .into_response()
}
