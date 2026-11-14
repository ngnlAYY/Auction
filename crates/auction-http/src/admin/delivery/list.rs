//! Admin 交付列表的处理器。

use std::net::SocketAddr;

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::delivery::DeliveryAdminListRequest;
use auction_contracts::{AdminDeliveryListResponse, AdminDeliveryPaginationResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_delivery_error, map_snapshot, no_store_headers, parse_optional_filter,
    parse_optional_reference, parse_optional_timestamp, parse_positive_i64, parse_query,
};
use crate::diagnostics;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 解析精确筛选条件；任一非法值都让整个请求失败关闭，不做部分忽略。
fn delivery_filters(
    query: &std::collections::BTreeMap<&str, String>,
) -> Result<DeliveryAdminListRequest, ()> {
    Ok(DeliveryAdminListRequest {
        recipient: parse_optional_filter(
            query,
            "recipient",
            auction_application::delivery::MAX_DELIVERY_ADMIN_RECIPIENT_LEN,
        )?,
        reference: parse_optional_reference(query, "reference")?,
        template: parse_optional_filter(
            query,
            "template",
            auction_application::delivery::MAX_DELIVERY_ADMIN_TEMPLATE_LEN,
        )?,
        category: query.get("category").cloned().filter(|value| !value.is_empty()),
        world: query.get("world").cloned().filter(|value| !value.is_empty()),
        created_from: parse_optional_timestamp(query, "created_from")?,
        created_to: parse_optional_timestamp(query, "created_to")?,
        ..DeliveryAdminListRequest::default()
    })
}

/// 读取 delivery Admin 列表。
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
    let query = match parse_query(
        &uri,
        &[
            "page",
            "page_size",
            "status",
            "recipient",
            "reference",
            "template",
            "category",
            "world",
            "created_from",
            "created_to",
        ],
    ) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(auction_application::delivery::MAX_DELIVERY_ADMIN_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) => offset,
        None => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let filters = match delivery_filters(&query) {
        Ok(filters) => filters,
        Err(()) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = DeliveryAdminListRequest {
        status: query.get("status").cloned(),
        limit: page_size,
        offset,
        ..filters
    };
    if !request.validate() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
            .into_response();
    }
    let Some(port) = state.delivery_admin() else {
        return feature_unavailable();
    };
    let page_result = match port.list(request).await {
        Ok(result) => result,
        Err(error) => return map_delivery_error(error),
    };
    let entry_count = match i64::try_from(page_result.entries.len()) {
        Ok(count) => count,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_delivery",
                "convert_entry_count",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || page_result.total < 0
        || entry_count > page_size
    {
        return internal_error();
    }
    let deliveries = page_result.entries.into_iter().map(map_snapshot).collect();
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "交付列表已加载",
            AdminDeliveryListResponse {
                deliveries,
                pagination: AdminDeliveryPaginationResponse {
                    page,
                    page_size,
                    total: page_result.total,
                },
            },
        )),
    )
        .into_response()
}
