//! Admin 结算报表选择目录 HTTP 适配器。

use std::net::SocketAddr;

use auction_application::admin::{
    MAX_SETTLEMENT_CATALOG_OFFSET, MAX_SETTLEMENT_CATALOG_PAGE_SIZE, SettlementCatalogRequest,
};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
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

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, feature_unavailable, invalid_query,
    storage_unavailable,
};
use crate::mapping::timestamp;
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

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
        Err(()) => return invalid_query("结算报表目录查询参数无效"),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(()) => return invalid_query("结算报表目录查询参数无效"),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_SETTLEMENT_CATALOG_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return invalid_query("结算报表目录查询参数无效"),
    };
    let Some(offset) = page_offset(page, page_size, Some(MAX_SETTLEMENT_CATALOG_OFFSET)) else {
        return invalid_query("结算报表目录查询参数无效");
    };
    let world_id = query.remove("world_id");
    let sequence = match query.get("sequence") {
        Some(value) => match value.parse::<i64>() {
            Ok(sequence) if sequence > 0 => Some(sequence),
            _ => return invalid_query("结算报表目录查询参数无效"),
        },
        None => None,
    };
    let request = SettlementCatalogRequest { world_id, sequence, limit: page_size, offset };
    if !request.validate() {
        return invalid_query("结算报表目录查询参数无效");
    }
    let Some(reader) = state.settlement_catalog() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let result = match reader.list_settlement_catalog(request.clone()).await {
        Ok(result) => result,
        Err(ApplicationError::CapabilityUnavailable(_)) => {
            return feature_unavailable(CAPABILITY_NOT_PROVIDED);
        }
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "admin_settlement",
                "read_catalog",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return storage_unavailable("结算报表目录暂不可用");
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        SettlementCatalogEntry, SettlementCatalogPage, SettlementCatalogReader,
        SettlementCatalogRequest,
    };
    use auction_application::core::ApplicationFuture;
    use auction_domain::AuctionId;
    use auction_time::Duration;
    use axum::extract::State;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use super::list;
    use crate::server::contract_fixtures::{
        AUCTION_ID, admin_address, admin_headers, admin_state, assert_http, time,
    };

    struct FixtureCatalog;

    impl SettlementCatalogReader for FixtureCatalog {
        fn list_settlement_catalog(
            &self,
            request: SettlementCatalogRequest,
        ) -> ApplicationFuture<'_, SettlementCatalogPage> {
            Box::pin(async move {
                let entries = [
                    SettlementCatalogEntry {
                        auction_id: AuctionId::from_uuid(AUCTION_ID),
                        world_id: "main".to_owned(),
                        sequence: 5,
                        settled_at: time() + Duration::microseconds(1),
                    },
                    SettlementCatalogEntry {
                        auction_id: AuctionId::from_uuid(Uuid::from_u128(
                            0x4000_8000_0000_0000_0020,
                        )),
                        world_id: "isekai".to_owned(),
                        sequence: 6,
                        settled_at: time(),
                    },
                ]
                .into_iter()
                .filter(|entry| {
                    request.world_id.as_ref().is_none_or(|world| world == &entry.world_id)
                })
                .collect::<Vec<_>>();
                Ok(SettlementCatalogPage {
                    total: i64::try_from(entries.len()).expect("场次数"),
                    entries,
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }
    }

    #[tokio::test]
    async fn settlement_catalog_contract_fixtures() {
        let state = admin_state().with_settlement_catalog(Arc::new(FixtureCatalog));
        for (name, uri) in [
            ("admin/settlement-catalog", "/api/v1/admin/settlement-catalog"),
            ("admin/settlement-catalog-world", "/api/v1/admin/settlement-catalog?world_id=isekai"),
        ] {
            let response = list(
                State(state.clone()),
                admin_address(),
                admin_headers(),
                uri.parse().expect("目录 URI"),
            )
            .await;
            assert_http(name, StatusCode::OK, response).await;
        }
    }
}
