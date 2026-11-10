//! Admin 改名历史 HTTP 适配器。
//! 该模块只负责旧契约的查询参数、分页和安全 DTO 映射；回滚命令由 Users owner 的
//! `admin_users` handler 持有，列表同时返回当前用户版本供页面建立 state-version fence。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminRenameListRequest, AdminRenameRecord, MAX_ADMIN_RENAME_OFFSET, MAX_ADMIN_RENAME_PAGE_SIZE,
};
use auction_application::core::ApplicationError;
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

    let mut query = match super::parse_query(&uri, &["user_id", "name", "page", "page_size"]) {
        Ok(query) => query,
        Err(_) => return super::invalid_query("查询参数无效"),
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_RENAME_PAGE_SIZE),
    ) {
        Ok(value) => value,
        Err(_) => return super::invalid_query("查询参数无效"),
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return super::invalid_query("查询参数无效"),
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= MAX_ADMIN_RENAME_OFFSET => offset,
        _ => return super::invalid_query("查询参数无效"),
    };
    let user_id = match query.get("user_id") {
        None => None,
        Some(value) => match Uuid::parse_str(value) {
            Ok(id) if !id.is_nil() => Some(id),
            _ => return super::invalid_query("查询参数无效"),
        },
    };
    let request = AdminRenameListRequest {
        user_id,
        name: query.remove("name").filter(|value| !value.is_empty()),
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return super::invalid_query("查询参数无效");
    }
    let Some(port) = state.admin_renames() else {
        return super::storage_unavailable("改名历史服务暂不可用");
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
        user_state_version: record.user_state_version,
        previous_username: record.previous_username,
        next_username: record.next_username,
        actor: record.actor,
        created_at: super::timestamp(record.created_at),
        reverted_at: record.reverted_at.map(super::timestamp),
    }
}

fn map_error(_error: ApplicationError) -> Response {
    super::storage_unavailable("改名历史服务暂不可用")
}
