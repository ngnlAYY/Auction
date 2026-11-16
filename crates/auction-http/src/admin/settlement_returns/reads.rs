//! 结算退件列表与详情的只读处理器；响应都是安全投影并带 `no-store`。

use std::net::SocketAddr;

use auction_contracts::ApiResponse;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::boundary::{feature_unavailable, map_settlement_return_error, parse_return_id};
use super::filters::{ListQuery, list_query};
use super::mapping::{map_detail, map_page};
use crate::errors::query_rejection;
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

/// 读取一页结算退件案件；隔离记录与未知义务也会列出。
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
    let Ok(ListQuery { page, request }) = list_query(&uri) else {
        return query_rejection().into_response();
    };
    let Some(port) = state.settlement_returns() else {
        return feature_unavailable();
    };
    let (limit, offset) = (request.limit, request.offset);
    let result = match port.list_settlement_returns(request).await {
        Ok(result) => result,
        Err(error) => return map_settlement_return_error("list", error),
    };
    let Some(response) = map_page(result, page, limit, offset) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("结算退件列表已加载", response)))
        .into_response()
}

/// 读取单个案件详情：事件、派发链、Take、代管、资产位置、允许动作与阻断原因。
pub(crate) async fn detail(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(return_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return query_rejection().into_response();
    }
    let return_id = match parse_return_id(&return_id) {
        Ok(return_id) => return_id,
        Err(response) => return *response,
    };
    let Some(port) = state.settlement_returns() else {
        return feature_unavailable();
    };
    let detail = match port.settlement_return_detail(return_id).await {
        Ok(detail) => detail,
        Err(error) => return map_settlement_return_error("detail", error),
    };
    let Some(response) = map_detail(detail, return_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("结算退件详情已加载", response)))
        .into_response()
}
