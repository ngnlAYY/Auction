//! Admin 改名历史 HTTP 适配器。
//!
//! 该模块只负责旧契约的查询参数、分页和安全 DTO 映射；改名回滚命令仍由后续迁移切片拥有。

use std::net::SocketAddr;

use auction_application::{
    AdminRenameListRequest, AdminRenameRecord, ApplicationError, MAX_ADMIN_RENAME_OFFSET,
    MAX_ADMIN_RENAME_PAGE_SIZE,
};
use auction_contracts::{
    AdminRenameListResponse, AdminRenamePaginationResponse, AdminRenameResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 Admin 改名历史。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }

    let query = match super::parse_query(&uri, &["user_id", "name", "page", "page_size"]) {
        Ok(query) => query,
        Err(_) => return invalid_query(),
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_RENAME_PAGE_SIZE),
    ) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= MAX_ADMIN_RENAME_OFFSET => offset,
        _ => return invalid_query(),
    };
    let user_id = match query.get("user_id") {
        None => None,
        Some(value) => match Uuid::parse_str(value) {
            Ok(id) if !id.is_nil() => Some(id),
            _ => return invalid_query(),
        },
    };
    let request = AdminRenameListRequest {
        user_id,
        name: query.get("name").filter(|value| !value.is_empty()).cloned(),
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return invalid_query();
    }
    let Some(port) = state.admin_renames() else {
        return storage_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }
    let response = AdminRenameListResponse {
        renames: result.entries.into_iter().map(map_record).collect(),
        pagination: AdminRenamePaginationResponse { page, page_size, total: result.total },
    };
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("改名历史已加载", response)))
        .into_response()
}

fn map_record(record: AdminRenameRecord) -> AdminRenameResponse {
    AdminRenameResponse {
        id: record.id.to_string(),
        user_id: record.user_id.to_string(),
        previous_username: record.previous_username,
        next_username: record.next_username,
        actor: record.actor,
        created_at: super::timestamp(record.created_at),
        reverted_at: record.reverted_at.map(super::timestamp),
    }
}

fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

fn storage_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "改名历史服务暂不可用")
        .into_response()
}

fn map_error(_error: ApplicationError) -> Response {
    storage_unavailable()
}
