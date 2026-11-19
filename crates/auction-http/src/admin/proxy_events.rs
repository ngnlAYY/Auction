//! Admin 代理历史 HTTP 适配器。
//! 这里仅处理 Admin 会话、路径/分页解析和稳定 DTO 映射；代理状态、命令账本和一致性快照
//! 由应用端口与存储适配器负责。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminProxyEvent, AdminProxyEventListRequest, AdminProxyEventPage, MAX_ADMIN_PROXY_EVENT_OFFSET,
    MAX_ADMIN_PROXY_EVENT_PAGE_SIZE,
};
use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::DiagnosticError;
use auction_contracts::{
    AdminProxyEventListResponse, AdminProxyEventPaginationResponse, AdminProxyEventResponse,
    ApiResponse,
};
use auction_domain::LotId;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error, feature_unavailable,
    query_rejection,
};
use crate::mapping::{decode_admin_proxy_event_cursor, encode_admin_proxy_event_cursor, timestamp};
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取指定 lot 的 Admin 代理历史。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }

    let query = match parse_query(&uri, &["page", "page_size", "cursor"]) {
        Ok(query) => query,
        Err(()) => return query_rejection().into_response(),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_PROXY_EVENT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return query_rejection().into_response(),
    };
    let raw_cursor = query.get("cursor").cloned();
    let (page, offset, cursor) = if let Some(raw_cursor) = raw_cursor.as_deref() {
        if query.contains_key("page") {
            return query_rejection().into_response();
        }
        let cursor = match decode_admin_proxy_event_cursor(raw_cursor) {
            Ok(cursor) => cursor,
            Err(()) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_cursor", "游标无效")
                    .into_response();
            }
        };
        (None, 0, Some(cursor))
    } else {
        let page = match parse_positive_i64(query.get("page"), 1, None) {
            Ok(page) => page,
            Err(()) => return query_rejection().into_response(),
        };
        let Some(offset) = page_offset(page, page_size, Some(MAX_ADMIN_PROXY_EVENT_OFFSET)) else {
            return query_rejection().into_response();
        };
        (Some(page), offset, None)
    };

    let Ok(lot_id) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "lot ID 无效").into_response();
    };
    if lot_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "lot ID 无效").into_response();
    }
    let request = AdminProxyEventListRequest {
        lot_id: LotId::from_uuid(lot_id),
        limit: page_size,
        offset,
        cursor,
    };
    let Some(port) = state.admin_proxy_events() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let Some(result) = (match port.list_admin_proxy_events(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    }) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "lot 不存在").into_response();
    };
    if !valid_page(&result, request) {
        return internal_error();
    }

    let response = if let Some(page) = page {
        AdminProxyEventListResponse {
            proxy_events: result.entries.into_iter().map(map_event).collect(),
            pagination: AdminProxyEventPaginationResponse {
                page: Some(page),
                page_size,
                total: Some(result.total),
                cursor: None,
                next_cursor: None,
                has_more: None,
            },
        }
    } else {
        AdminProxyEventListResponse {
            proxy_events: result.entries.into_iter().map(map_event).collect(),
            pagination: AdminProxyEventPaginationResponse {
                page: None,
                page_size,
                total: None,
                cursor: raw_cursor,
                next_cursor: result.next_cursor.map(encode_admin_proxy_event_cursor),
                has_more: Some(result.has_more),
            },
        }
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理历史已加载", response)))
        .into_response()
}

fn valid_page(page: &AdminProxyEventPage, request: AdminProxyEventListRequest) -> bool {
    page.validate(request)
}

fn map_event(event: AdminProxyEvent) -> AdminProxyEventResponse {
    AdminProxyEventResponse {
        authorization_id: event.authorization_id.to_string(),
        authorization_sequence: event.authorization_sequence,
        command_id: event.command_id.to_string(),
        created_at: timestamp(event.created_at),
        lot_id: event.lot_id.to_string(),
        maximum_amount: event.maximum_amount,
        mode: event.mode,
        state_version: event.state_version,
        // `final_fast` 是新版内部状态；旧 v1 Admin wire 只认识 active，不能把内部建模泄漏给旧客户端。
        status: if event.status == "final_fast" { "active".to_owned() } else { event.status },
        user_id: event.user_id.to_string(),
        usage_number: event.usage_number,
        username: event.username,
    }
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(CAPABILITY_NOT_PROVIDED),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 代理历史服务暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_proxy_events",
        "respond_read_failure",
        diagnostic_kind,
        response,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminProxyEvent, AdminProxyEventCursor, AdminProxyEventListRequest, AdminProxyEventPage,
        AdminProxyEventReader, UNKNOWN_ADMIN_PROXY_EVENT_TOTAL,
    };
    use auction_application::core::ApplicationFuture;
    use auction_domain::LotId;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use crate::mapping::encode_admin_proxy_event_cursor;
    use crate::server::contract_fixtures::{AdminFixturePorts, admin_get, assert_http, time};

    const LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);

    struct FixtureProxyEvents {
        finalized: bool,
    }

    impl AdminProxyEventReader for FixtureProxyEvents {
        fn list_admin_proxy_events(
            &self,
            request: AdminProxyEventListRequest,
        ) -> ApplicationFuture<'_, Option<AdminProxyEventPage>> {
            let lot_id = LotId::from_uuid(LOT_ID);
            assert_eq!(request.lot_id, lot_id);
            let cursor_mode = request.cursor.is_some();
            let mode = if self.finalized || cursor_mode { "fast" } else { "slow" };
            let event = AdminProxyEvent {
                command_id: Uuid::from_u128(0x4000_8000_0000_0000_0028),
                authorization_id: Uuid::from_u128(0x4000_8000_0000_0000_0029),
                authorization_sequence: 1,
                usage_number: 1,
                lot_id,
                user_id: Uuid::from_u128(0x4000_8000_0000_0000_0024),
                username: "ExamplePlayer".to_owned(),
                mode: mode.to_owned(),
                maximum_amount: 5_000,
                status: if self.finalized { "final_fast" } else { "active" }.to_owned(),
                state_version: if self.finalized { 2 } else { 1 },
                created_at: time(),
            };
            Box::pin(async move {
                Ok(Some(AdminProxyEventPage {
                    lot_id,
                    entries: vec![event],
                    total: if cursor_mode { UNKNOWN_ADMIN_PROXY_EVENT_TOTAL } else { 1 },
                    limit: request.limit,
                    offset: request.offset,
                    cursor: request.cursor,
                    next_cursor: None,
                    has_more: false,
                }))
            })
        }
    }

    #[tokio::test]
    async fn proxy_events_contract_fixtures() {
        let path = format!("/api/v1/admin/lots/{LOT_ID}/proxy-events");
        let cursor = encode_admin_proxy_event_cursor(AdminProxyEventCursor::Start);
        let cursor_path = format!("{path}?cursor={cursor}");
        for (name, url, finalized) in [
            ("admin/proxy-events", &path, false),
            ("admin/proxy-events-final", &path, true),
            ("admin/proxy-events-cursor", &cursor_path, false),
        ] {
            let state = AdminFixturePorts {
                proxy_events: Some(Arc::new(FixtureProxyEvents { finalized })),
                ..Default::default()
            }
            .state();
            assert_http(name, StatusCode::OK, admin_get(state, url).await).await;
        }
    }
}
