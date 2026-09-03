use std::net::SocketAddr;

use auction_application::{
    AdminAuctionListRequest, MAX_ADMIN_AUCTION_OFFSET, MAX_ADMIN_AUCTION_PAGE_SIZE,
};
use auction_contracts::{
    AdminAuctionCapabilitiesResponse, AdminAuctionListResponse, AdminAuctionPaginationResponse,
    ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, feature_unavailable,
    internal_error, map_auction, map_error, no_store_headers, parse_positive_i64, parse_query,
};

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 Admin 拍卖列表。
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

    let query = match parse_query(&uri, &["world_id", "status", "page", "page_size"]) {
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
        Some(MAX_ADMIN_AUCTION_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= MAX_ADMIN_AUCTION_OFFSET => offset,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = AdminAuctionListRequest {
        world_id: query.get("world_id").cloned(),
        status: query.get("status").cloned(),
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
            .into_response();
    }
    let Some(port) = state.admin_auctions() else {
        return feature_unavailable();
    };
    let result = match port.list_admin_auctions(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return internal_error();
    }

    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "拍卖列表已加载",
            AdminAuctionListResponse {
                auctions: result.entries.into_iter().map(map_auction).collect(),
                capabilities: AdminAuctionCapabilitiesResponse {
                    cancel_auction: true,
                    manual_early_open: state
                        .lifecycle()
                        .is_some_and(|lifecycle| lifecycle.allows_manual_early_open()),
                    manual_early_settlement: state
                        .lifecycle()
                        .is_some_and(|lifecycle| lifecycle.allows_manual_early_settlement()),
                    manual_intake: state.admin_intake().is_some(),
                    manual_preview_open: true,
                    modify_schedule: state.auction_schedule().is_some(),
                },
                pagination: AdminAuctionPaginationResponse { page, page_size, total: result.total },
            },
        )),
    )
        .into_response()
}
