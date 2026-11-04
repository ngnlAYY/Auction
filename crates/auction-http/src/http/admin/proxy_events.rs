//! Admin 代理历史 HTTP 适配器。
//! 这里仅处理 Admin 会话、路径/分页解析和稳定 DTO 映射；代理状态、命令账本和一致性快照
//! 由应用端口与存储适配器负责。

use auction_application::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::{
    AdminProxyEvent, AdminProxyEventCursor, AdminProxyEventListRequest, AdminProxyEventPage,
    ApplicationError, MAX_ADMIN_PROXY_EVENT_OFFSET, MAX_ADMIN_PROXY_EVENT_PAGE_SIZE,
};
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
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;
const CURSOR_VERSION: u8 = 1;
const CURSOR_KIND: u8 = 7;
const CURSOR_BYTES: usize = 26;

/// 读取指定 lot 的 Admin 代理历史。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }

    let query = match super::parse_query(&uri, &["page", "page_size", "cursor"]) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_PROXY_EVENT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let raw_cursor = query.get("cursor").cloned();
    let (page, offset, cursor) = if let Some(raw_cursor) = raw_cursor.as_deref() {
        if query.contains_key("page") {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
        let cursor = match decode_cursor(raw_cursor) {
            Ok(cursor) => cursor,
            Err(()) => {
                return super::api_error(StatusCode::BAD_REQUEST, "invalid_cursor", "游标无效")
                    .into_response();
            }
        };
        (None, 0, Some(cursor))
    } else {
        let page = match super::parse_positive_i64(query.get("page"), 1, None) {
            Ok(page) => page,
            Err(_) => {
                return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        };
        let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
            Some(offset) if offset <= MAX_ADMIN_PROXY_EVENT_OFFSET => offset,
            _ => {
                return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        };
        (Some(page), offset, None)
    };

    let Ok(lot_id) = Uuid::parse_str(&lot_id) else {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "lot ID 无效")
            .into_response();
    };
    if lot_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "lot ID 无效")
            .into_response();
    }
    let request = AdminProxyEventListRequest {
        lot_id: LotId::from_uuid(lot_id),
        limit: page_size,
        offset,
        cursor,
    };
    let Some(port) = state.admin_proxy_events() else {
        return feature_unavailable();
    };
    let Some(result) = (match port.list_admin_proxy_events(request).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    }) else {
        return super::api_error(StatusCode::NOT_FOUND, "not_found", "lot 不存在").into_response();
    };
    if !valid_page(&result, request) {
        return super::internal_error();
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
                next_cursor: result.next_cursor.map(encode_cursor),
                has_more: Some(result.has_more),
            },
        }
    };
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("代理历史已加载", response)))
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
        created_at: super::timestamp(event.created_at),
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

fn encode_cursor(cursor: AdminProxyEventCursor) -> String {
    let AdminProxyEventCursor::Boundary { created_at, command_id } = cursor else {
        return "start".to_owned();
    };
    let mut bytes = [0_u8; CURSOR_BYTES];
    bytes[0] = CURSOR_VERSION;
    bytes[1] = CURSOR_KIND;
    bytes[2..10].copy_from_slice(&created_at.timestamp_micros().to_be_bytes());
    bytes[10..].copy_from_slice(command_id.as_bytes());
    URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_cursor(value: &str) -> Result<AdminProxyEventCursor, ()> {
    if value == "start" {
        return Ok(AdminProxyEventCursor::Start);
    }
    // 固定 26 字节的旧游标只可能编码为 35 个无填充字符；先拒绝超长输入，再解码到栈。
    if value.len() != (CURSOR_BYTES * 4).div_ceil(3) {
        return Err(());
    }
    let mut bytes = [0_u8; CURSOR_BYTES];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(value.as_bytes(), &mut bytes).map_err(|_| ())?;
    if decoded_len != CURSOR_BYTES || bytes[0] != CURSOR_VERSION || bytes[1] != CURSOR_KIND {
        return Err(());
    }
    let mut timestamp = [0_u8; 8];
    timestamp.copy_from_slice(&bytes[2..10]);
    let created_at = Utc.timestamp_micros(i64::from_be_bytes(timestamp)).single().ok_or(())?;
    let command_id = Uuid::from_slice(&bytes[10..]).map_err(|_| ())?;
    let cursor = AdminProxyEventCursor::Boundary { created_at, command_id };
    cursor.validate().then_some(cursor).ok_or(())
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 代理历史仅支持 PostgreSQL 完整档",
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
