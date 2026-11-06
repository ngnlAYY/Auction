//! Admin lot 列表 HTTP 适配器。
//! 认证和应用端口负责权限、快照与存储语义；这里仅解析路径/分页并映射安全响应，不把 SQL
//! 行或装备密钥传到浏览器。

use auction_application::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::{
    AdminAuctionSummary, AdminLotListRequest, AdminLotPage, AdminLotSummary, ApplicationError,
};
use auction_contracts::{
    AdminLotListResponse, AdminLotPaginationResponse, AdminLotResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取指定拍卖的 Admin lot 页面。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(&uri, &["page", "page_size"]) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(auction_application::MAX_ADMIN_LOT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= auction_application::MAX_ADMIN_LOT_OFFSET => offset,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let Ok(auction_id) = Uuid::parse_str(&auction_id) else {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    let request = AdminLotListRequest { auction_id: auction_id.into(), limit: page_size, offset };
    let Some(port) = state.admin_lots() else {
        return feature_unavailable();
    };
    let Some(result) = (match port.list_admin_lots(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    }) else {
        return super::api_error(StatusCode::NOT_FOUND, "not_found", "拍卖不存在").into_response();
    };
    if !valid_page(&result, request) {
        return super::internal_error();
    }
    let total = result.total;
    let mut response_headers = super::no_store_headers();
    response_headers.insert(
        "x-auction-capability-manual-intake",
        HeaderValue::from_static(if state.admin_intake().is_some() { "true" } else { "false" }),
    );
    (
        StatusCode::OK,
        response_headers,
        Json(ApiResponse::ok(
            "后台 lot 列表已加载",
            AdminLotListResponse {
                auction: super::auctions::map_auction(AdminAuctionSummary {
                    auction: result.auction,
                    scheduled_ends_at: None,
                    lot_count: total,
                }),
                lots: result.entries.into_iter().map(map_lot).collect(),
                pagination: AdminLotPaginationResponse { page, page_size, total },
            },
        )),
    )
        .into_response()
}

fn valid_page(page: &AdminLotPage, request: AdminLotListRequest) -> bool {
    page.validate(request)
        && page.auction.sequence > 0
        && !page.auction.world_id.is_empty()
        && !page.auction.status.is_empty()
        && page.auction.preview_scheduled_at <= page.auction.scheduled_open_at
        && page.auction.preview_revision >= 0
        && page.auction.state_version > 0
        && page.auction.created_at <= page.auction.updated_at
        && (page.auction.status == "cancelled"
            || (page.auction.cancelled_at.is_none()
                && page.auction.cancelled_by.is_none()
                && page.auction.cancelled_reason.is_none()))
}

pub(crate) fn map_lot(lot: AdminLotSummary) -> AdminLotResponse {
    AdminLotResponse {
        id: lot.id.to_string(),
        auction_id: lot.auction_id.to_string(),
        equipment_id: lot.equipment_id.to_string(),
        submission_id: lot.submission_id.map(|id| id.to_string()),
        lot_number: lot.lot_number,
        status: lot.status,
        start_price: lot.start_price,
        current_price: lot.current_price,
        leader_user_id: lot.leader_user_id.map(|id| id.to_string()),
        effective_end_at: lot.effective_end_at.map(super::timestamp),
        slow_due_at: lot.slow_due_at.map(super::timestamp),
        slow_generation: lot.slow_generation,
        slow_authorization_id: lot.slow_authorization_id.map(|id| id.to_string()),
        state_version: lot.state_version,
        created_at: super::timestamp(lot.created_at),
        updated_at: super::timestamp(lot.updated_at),
        external_equipment_id: lot.external_equipment_id,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        equipment_attributes: lot.equipment_attributes,
        equipment_collected_at: super::timestamp(lot.equipment_collected_at),
        seller: lot.seller,
        submission_source_kind: lot.submission_source_kind,
        submission_source_reference: lot.submission_source_reference,
        mailbox_key: lot.mailbox_key,
        external_mail_id: lot.external_mail_id,
        submission_received_at: lot.submission_received_at.map(super::timestamp),
    }
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin lot 列表仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin lot 服务暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response("admin_lots", "respond_list_failure", diagnostic_kind, response)
}
