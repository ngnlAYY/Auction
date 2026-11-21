//! Admin 拍卖列表的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
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

use super::{feature_unavailable, map_auction, map_error};
use crate::errors::query_rejection;
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

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

    let mut query = match parse_query(&uri, &["world_id", "status", "page", "page_size"]) {
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
        Some(MAX_ADMIN_AUCTION_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return query_rejection().into_response(),
    };
    let Some(offset) = page_offset(page, page_size, Some(MAX_ADMIN_AUCTION_OFFSET)) else {
        return query_rejection().into_response();
    };
    let request = AdminAuctionListRequest {
        world_id: query.remove("world_id"),
        status: query.remove("status"),
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return query_rejection().into_response();
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
                        .is_some_and(auction_application::auction::AuctionLifecycleCommandPort::allows_manual_early_open),
                    manual_early_settlement: state
                        .lifecycle()
                        .is_some_and(auction_application::auction::AuctionLifecycleCommandPort::allows_manual_early_settlement),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminAuctionCreatePort, AdminAuctionPage, AdminAuctionReader, AdminAuctionSchedulePort,
        AdminAuctionSummary, AdminCreateAuctionCommand, AdminUpdateAuctionScheduleCommand,
    };
    use auction_application::core::ApplicationFuture;
    use axum::http::Method;
    use serde_json::json;
    use uuid::Uuid;

    use super::{AdminAuctionListRequest, StatusCode};
    use crate::server::contract_fixtures::{
        AdminFixturePorts, admin_get, admin_request, assert_http, draft_auction,
    };

    struct Auctions;

    impl AdminAuctionReader for Auctions {
        fn list_admin_auctions(
            &self,
            request: AdminAuctionListRequest,
        ) -> ApplicationFuture<'_, AdminAuctionPage> {
            Box::pin(async move {
                let filtered = request.offset == 1;
                assert_eq!(request.world_id.as_deref(), filtered.then_some("main"));
                assert_eq!(request.status.as_deref(), filtered.then_some("draft"));
                let auction = draft_auction();
                let scheduled_ends_at =
                    Some(auction.scheduled_open_at + auction_time::Duration::hours(1));
                Ok(AdminAuctionPage {
                    entries: if request.offset == 0 {
                        vec![AdminAuctionSummary { auction, scheduled_ends_at, lot_count: 0 }]
                    } else {
                        Vec::new()
                    },
                    total: 1,
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }
    }

    impl AdminAuctionCreatePort for Auctions {
        fn create(
            &self,
            command: AdminCreateAuctionCommand,
        ) -> ApplicationFuture<'_, AdminAuctionSummary> {
            Box::pin(async move {
                let auction = auction_application::settlement::SettlementAuctionSnapshot {
                    id: command.auction_id,
                    sequence: command.sequence,
                    world_id: command.world_id,
                    preview_scheduled_at: command.preview_scheduled_at,
                    scheduled_open_at: command.scheduled_open_at,
                    ..draft_auction()
                };
                Ok(AdminAuctionSummary {
                    auction,
                    scheduled_ends_at: Some(command.scheduled_end_at),
                    lot_count: 0,
                })
            })
        }
    }
    impl AdminAuctionSchedulePort for Auctions {
        fn update_schedule(
            &self,
            command: AdminUpdateAuctionScheduleCommand,
        ) -> ApplicationFuture<'_, AdminAuctionSummary> {
            Box::pin(async move {
                assert!(command.validate());
                let previous = draft_auction();
                assert_eq!(command.auction_id, previous.id);
                assert_eq!(command.expected_state_version, previous.state_version);
                let auction = auction_application::settlement::SettlementAuctionSnapshot {
                    id: command.auction_id,
                    preview_scheduled_at: command.preview_scheduled_at,
                    scheduled_open_at: command.scheduled_open_at,
                    updated_at: previous.created_at + auction_time::Duration::minutes(1),
                    state_version: previous.state_version + 1,
                    ..previous
                };
                Ok(AdminAuctionSummary {
                    auction,
                    scheduled_ends_at: Some(command.scheduled_end_at),
                    lot_count: 0,
                })
            })
        }
    }

    #[tokio::test]
    async fn auction_contract_fixtures() {
        let state = AdminFixturePorts {
            auctions: Some(Arc::new(Auctions)),
            auction_create: Some(Arc::new(Auctions)),
            ..Default::default()
        }
        .state();
        for (name, path) in [
            ("admin/auctions", "/api/v1/admin/auctions"),
            (
                "admin/auctions-empty-page",
                "/api/v1/admin/auctions?world_id=main&status=draft&page=2&page_size=1",
            ),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), path).await).await;
        }
        let auction = draft_auction();
        let body = json!({
            "command_id": Uuid::from_u128(7),
            "auction_id": "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a",
            "sequence": auction.sequence, "world_id": auction.world_id,
            "preview_scheduled_at": auction.preview_scheduled_at,
            "scheduled_open_at": auction.scheduled_open_at,
            "scheduled_end_at": auction.scheduled_open_at + auction_time::Duration::hours(1),
            "expected_state_version": 0,
        });
        let response = admin_request(
            state.clone(),
            Method::POST,
            "/api/v1/admin/auctions",
            body.to_string().into(),
        )
        .await;
        assert_http("admin/auction-create-command", StatusCode::CREATED, response).await;

        let state = state.with_auction_schedule(Arc::new(Auctions));
        let body = json!({
            "command_id": "00000000-0000-4000-8000-000000000008",
            "preview_scheduled_at": auction.preview_scheduled_at,
            "scheduled_open_at": auction.scheduled_open_at,
            "scheduled_end_at": auction.scheduled_open_at + auction_time::Duration::hours(2),
            "expected_state_version": 1,
        });
        let path = format!("/api/v1/admin/auctions/{}/schedule", auction.id);
        let response = admin_request(state, Method::PATCH, &path, body.to_string().into()).await;
        assert_http("admin/auction-schedule-command", StatusCode::OK, response).await;
    }
}
