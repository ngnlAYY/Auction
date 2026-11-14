//! Admin realtime 事件、lag 与 failure 列表的处理器。

use std::net::SocketAddr;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::realtime::{
    RealtimeEventListRequest, RealtimeFailureListRequest, RealtimeLagListRequest,
};
use auction_contracts::{
    AdminRealtimeEventListResponse, AdminRealtimeFailureListResponse,
    AdminRealtimeFailurePaginationResponse, AdminRealtimeLagListResponse,
    AdminRealtimePaginationResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_event, map_failure, map_lag, map_realtime_error, no_store_headers,
    parse_page, parse_query,
};
use crate::diagnostics;

/// 读取已持久化 realtime 事件的安全元数据。
pub(crate) async fn list_events(
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
    let query = match parse_query(&uri, &["stream_key", "kind", "page", "page_size"]) {
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
        .list_events(RealtimeEventListRequest {
            stream_key: query.get("stream_key").cloned(),
            kind: query.get("kind").cloned(),
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
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_realtime",
                "convert_event_count",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let expected_count = result.total.saturating_sub(offset).clamp(0, page_size);
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || entry_count != expected_count
    {
        return internal_error();
    }
    let events = result.entries.into_iter().map(map_event).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "实时事件元数据已加载",
            AdminRealtimeEventListResponse {
                events,
                pagination: AdminRealtimePaginationResponse {
                    page,
                    page_size,
                    total: result.total,
                },
            },
        )),
    )
        .into_response()
}

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
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_realtime",
                "convert_lag_count",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
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
            AdminRealtimeLagListResponse {
                lags,
                pagination: AdminRealtimePaginationResponse {
                    page,
                    page_size,
                    total: result.total,
                },
            },
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
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_realtime",
                "convert_failure_count",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || entry_count > page_size
    {
        return internal_error();
    }
    // 旧 Admin wire 按完整页边界推导 has_more；不能在短页时改用实际返回条数制造协议漂移。
    let has_more = page.checked_mul(page_size).is_some_and(|end| end < result.total);
    let failures = result.entries.into_iter().map(map_failure).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费失败列表已加载",
            AdminRealtimeFailureListResponse {
                failures,
                pagination: AdminRealtimeFailurePaginationResponse {
                    page,
                    page_size,
                    has_more,
                    total: result.total,
                },
            },
        )),
    )
        .into_response()
}
