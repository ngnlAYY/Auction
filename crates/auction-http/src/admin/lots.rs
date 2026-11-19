//! Admin lot 列表 HTTP 适配器。
//! 认证和应用端口负责权限、快照与存储语义；这里仅解析路径/分页并映射安全响应，不把 SQL
//! 行或装备密钥传到浏览器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminAuctionSummary, AdminLotListRequest, AdminLotPage, AdminLotSummary,
};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
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

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error, feature_unavailable,
    query_rejection,
};
use crate::mapping::timestamp;
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取指定拍卖的 Admin lot 页面。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match parse_query(&uri, &["page", "page_size"]) {
        Ok(query) => query,
        Err(()) => return query_rejection().into_response(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(()) => return query_rejection().into_response(),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(auction_application::admin::MAX_ADMIN_LOT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return query_rejection().into_response(),
    };
    let Some(offset) =
        page_offset(page, page_size, Some(auction_application::admin::MAX_ADMIN_LOT_OFFSET))
    else {
        return query_rejection().into_response();
    };
    let Ok(auction_id) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    let request = AdminLotListRequest { auction_id: auction_id.into(), limit: page_size, offset };
    let Some(port) = state.admin_lots() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let Some(result) = (match port.list_admin_lots(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    }) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "拍卖不存在").into_response();
    };
    if !valid_page(&result, request) {
        return internal_error();
    }
    let total = result.total;
    let mut response_headers = no_store_headers();
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
        effective_end_at: lot.effective_end_at.map(timestamp),
        slow_due_at: lot.slow_due_at.map(timestamp),
        slow_generation: lot.slow_generation,
        slow_authorization_id: lot.slow_authorization_id.map(|id| id.to_string()),
        state_version: lot.state_version,
        created_at: timestamp(lot.created_at),
        updated_at: timestamp(lot.updated_at),
        external_equipment_id: lot.external_equipment_id,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        equipment_attributes: lot.equipment_attributes,
        equipment_collected_at: timestamp(lot.equipment_collected_at),
        seller: lot.seller,
        submission_source_kind: lot.submission_source_kind,
        submission_source_reference: lot.submission_source_reference,
        mailbox_key: lot.mailbox_key,
        external_mail_id: lot.external_mail_id,
        submission_received_at: lot.submission_received_at.map(timestamp),
    }
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(CAPABILITY_NOT_PROVIDED),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin lot 服务暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response("admin_lots", "respond_list_failure", diagnostic_kind, response)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{AdminLotReader, AdminLotUpdateCommand, AdminLotUpdatePort};
    use auction_application::core::ApplicationFuture;
    use axum::http::Method;
    use serde_json::json;

    use super::{AdminLotListRequest, AdminLotPage, AdminLotSummary, StatusCode, Uuid};
    use crate::server::contract_fixtures::{
        AUCTION_ID, AdminFixturePorts, admin_get, admin_request, assert_http, draft_auction, time,
    };

    const LOT_ID: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0023);

    fn lot() -> AdminLotSummary {
        AdminLotSummary {
            id: LOT_ID.into(),
            auction_id: AUCTION_ID.into(),
            equipment_id: Uuid::from_u128(0x4000_8000_0000_0000_0004).into(),
            submission_id: Some(Uuid::from_u128(0x4000_8000_0000_0000_0006).into()),
            lot_number: 1,
            status: "pending".to_owned(),
            start_price: 1000,
            current_price: 1000,
            leader_user_id: None,
            effective_end_at: None,
            slow_due_at: None,
            slow_generation: 0,
            slow_authorization_id: None,
            state_version: 1,
            created_at: time(),
            updated_at: time(),
            external_equipment_id: "123".to_owned(),
            equipment_name: "Legendary Axe".to_owned(),
            equipment_category: "weapon".to_owned(),
            equipment_attributes: json!({}),
            equipment_collected_at: time(),
            seller: Some("Seller".to_owned()),
            submission_source_kind: Some("mail".to_owned()),
            submission_source_reference: None,
            mailbox_key: Some("main".to_owned()),
            external_mail_id: Some("9001".to_owned()),
            submission_received_at: Some(time()),
        }
    }

    struct Lots;

    impl AdminLotReader for Lots {
        fn list_admin_lots(
            &self,
            request: AdminLotListRequest,
        ) -> ApplicationFuture<'_, Option<AdminLotPage>> {
            Box::pin(async move {
                assert_eq!(request.auction_id, AUCTION_ID.into());
                Ok(Some(AdminLotPage {
                    auction: auction_application::settlement::SettlementAuctionSnapshot {
                        preview_revision: 1,
                        state_version: 2,
                        ..draft_auction()
                    },
                    entries: if request.offset == 0 { vec![lot()] } else { Vec::new() },
                    total: 1,
                    limit: request.limit,
                    offset: request.offset,
                }))
            })
        }
    }

    impl AdminLotUpdatePort for Lots {
        fn update(&self, command: AdminLotUpdateCommand) -> ApplicationFuture<'_, AdminLotSummary> {
            Box::pin(async move {
                assert_eq!(command.lot_id, LOT_ID.into());
                assert_eq!(command.expected_state_version, 1);
                let price = command.start_price.expect("修改后的起拍价");
                Ok(AdminLotSummary {
                    start_price: price,
                    current_price: price,
                    state_version: 2,
                    updated_at: time() + auction_time::Duration::minutes(1),
                    ..lot()
                })
            })
        }
    }

    #[tokio::test]
    async fn lot_contract_fixtures() {
        let state = AdminFixturePorts {
            lots: Some(Arc::new(Lots)),
            lot_update: Some(Arc::new(Lots)),
            ..Default::default()
        }
        .state();
        for (name, query) in [("admin/lots", ""), ("admin/lots-empty-page", "?page=2&page_size=1")]
        {
            let path = format!("/api/v1/admin/auctions/{AUCTION_ID}/lots{query}");
            assert_http(name, StatusCode::OK, admin_get(state.clone(), &path).await).await;
        }
        let path = format!("/api/v1/admin/lots/{LOT_ID}");
        let body = json!({"command_id": Uuid::from_u128(0x4000_8000_0000_0000_0007), "expected_state_version": 1, "action": "set_start_price", "start_price": 2000});
        let response = admin_request(state, Method::PATCH, &path, body.to_string().into()).await;
        assert_http("admin/lot-update-command", StatusCode::OK, response).await;
    }
}
