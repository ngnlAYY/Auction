//! Admin 结算报表选择目录 HTTP 适配器。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::admin::{
    MAX_SETTLEMENT_CATALOG_OFFSET, MAX_SETTLEMENT_CATALOG_PAGE_SIZE, SettlementCatalogRequest,
};
use auction_application::core::ApplicationError;
use auction_contracts::{
    AdminSettlementCatalogEntryResponse, AdminSettlementCatalogPaginationResponse,
    AdminSettlementCatalogResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_session, internal_error,
    no_store_headers, parse_positive_i64, parse_query, timestamp,
};

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取只包含已结算拍卖的报表选择目录。
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

    let mut query = match parse_query(&uri, &["world_id", "sequence", "page", "page_size"]) {
        Ok(query) => query,
        Err(_) => return invalid_query(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => return invalid_query(),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_SETTLEMENT_CATALOG_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => return invalid_query(),
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= MAX_SETTLEMENT_CATALOG_OFFSET => offset,
        _ => return invalid_query(),
    };
    let world_id = query.remove("world_id");
    let sequence = match query.get("sequence") {
        Some(value) => match value.parse::<i64>() {
            Ok(sequence) if sequence > 0 => Some(sequence),
            _ => return invalid_query(),
        },
        None => None,
    };
    let request = SettlementCatalogRequest { world_id, sequence, limit: page_size, offset };
    if !request.validate() {
        return invalid_query();
    }
    let Some(reader) = state.settlement_catalog() else {
        return feature_unavailable();
    };
    let result = match reader.list_settlement_catalog(request.clone()).await {
        Ok(result) => result,
        Err(ApplicationError::CapabilityUnavailable(_)) => return feature_unavailable(),
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "admin_settlement",
                "read_catalog",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return unavailable();
        }
    };
    if !result.validate(&request) {
        return internal_error();
    }
    let auctions = result
        .entries
        .into_iter()
        .map(|entry| AdminSettlementCatalogEntryResponse {
            auction_id: entry.auction_id.to_string(),
            world_id: entry.world_id,
            sequence: entry.sequence,
            settled_at: timestamp(entry.settled_at),
        })
        .collect();

    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "结算报表目录已加载",
            AdminSettlementCatalogResponse {
                auctions,
                pagination: AdminSettlementCatalogPaginationResponse {
                    page,
                    page_size,
                    total: result.total,
                },
            },
        )),
    )
        .into_response()
}

fn invalid_query() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_query", "结算报表目录查询参数无效").into_response()
}

fn feature_unavailable() -> Response {
    api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "结算报表目录仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn unavailable() -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "结算报表目录暂不可用")
        .into_response()
}
