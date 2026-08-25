use std::net::SocketAddr;

use auction_application::{RealtimeFailureListRequest, RealtimeLagListRequest};
use auction_contracts::{
    AdminRealtimeFailureDetailResponse, AdminRealtimeFailureListResponse,
    AdminRealtimeLagListResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_failure, map_lag, map_realtime_error, no_store_headers, not_found_error,
    parse_page, parse_query, parse_uuid,
};

/// 读取 realtime consumer lag。
pub(crate) async fn list_lag(
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
    let query = match parse_query(&uri, &["stream_prefix", "consumer_key", "page", "page_size"]) {
        Ok(query) => query,
        Err(_) => return super::invalid_query(),
    };
    let (page, page_size, offset) = match parse_page(&query) {
        Ok(page) => page,
        Err(_) => return super::invalid_query(),
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let result = match port
        .list_lag(RealtimeLagListRequest {
            stream_prefix: query.get("stream_prefix").cloned(),
            consumer_key: query.get("consumer_key").cloned(),
            limit: page_size,
            offset,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_realtime_error(error),
    };
    let entry_count = match i64::try_from(result.entries.len()) {
        Ok(count) => count,
        Err(_) => return internal_error(),
    };
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || entry_count > page_size
    {
        return internal_error();
    }
    let lags = result.entries.into_iter().map(map_lag).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费延迟已加载",
            AdminRealtimeLagListResponse { lags, page, page_size, total: result.total },
        )),
    )
        .into_response()
}

/// 读取 realtime consumer failure 列表。
pub(crate) async fn list_failures(
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
    let query = match parse_query(
        &uri,
        &["stream_key", "consumer_key", "code", "resolved", "page", "page_size"],
    ) {
        Ok(query) => query,
        Err(_) => return super::invalid_query(),
    };
    let resolved = match query.get("resolved") {
        None => None,
        Some(value) => Some(match value.as_str() {
            "true" => true,
            "false" => false,
            _ => return super::invalid_query(),
        }),
    };
    let (page, page_size, offset) = match parse_page(&query) {
        Ok(page) => page,
        Err(_) => return super::invalid_query(),
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let result = match port
        .list_failures(RealtimeFailureListRequest {
            stream_key: query.get("stream_key").cloned(),
            consumer_key: query.get("consumer_key").cloned(),
            code: query.get("code").cloned(),
            resolved,
            limit: page_size,
            offset,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_realtime_error(error),
    };
    let entry_count = match i64::try_from(result.entries.len()) {
        Ok(count) => count,
        Err(_) => return internal_error(),
    };
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || entry_count > page_size
    {
        return internal_error();
    }
    let has_more = offset.checked_add(entry_count).is_some_and(|next| next < result.total);
    let failures = result.entries.into_iter().map(map_failure).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费失败列表已加载",
            AdminRealtimeFailureListResponse {
                failures,
                page,
                page_size,
                has_more,
                total: result.total,
            },
        )),
    )
        .into_response()
}

/// 读取单个 realtime failure 的安全详情。
pub(crate) async fn get_failure(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(failure_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let failure_id = match parse_uuid(&failure_id, "failure_id") {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let failure = match port.get_failure(failure_id).await {
        Ok(Some(failure)) => failure,
        Ok(None) => return not_found_error().into_response(),
        Err(error) => return map_realtime_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费失败详情已加载",
            AdminRealtimeFailureDetailResponse { failure: map_failure(failure) },
        )),
    )
        .into_response()
}
