//! Admin 论坛本地投影读取；刷新不调用论坛网关，也不授予 Worker 写入权限。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_contracts::{ApiResponse, PublicForumResponse, PublicForumSyncStatusResponse};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_session, no_store_headers,
    parse_canonical_uuid, parse_query, timestamp,
};

async fn authorize(
    state: &AdminApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    uri: &Uri,
) -> Result<(), Box<Response>> {
    if let Some(response) = admin_session_guard(state, headers, address, false, false) {
        return Err(Box::new(response));
    }
    authenticated_admin_session(state, headers).await?;
    parse_query(uri, &[]).map_err(|_| {
        Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response(),
        )
    })?;
    Ok(())
}

/// 在 Admin 会话内读取 Worker 已公布的同步开关；未知不能伪装成关闭。
pub(crate) async fn sync_status(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authorize(&state, &headers, address, &uri).await {
        return *response;
    }
    let Some(reader) = state.forum_reader() else {
        return super::super::storage_unavailable("论坛同步状态暂不可用");
    };
    match reader.public_forum_sync_enabled().await {
        Ok(Some(enabled)) => (
            no_store_headers(),
            Json(ApiResponse::ok(
                "论坛同步启用状态已加载",
                PublicForumSyncStatusResponse { enabled },
            )),
        )
            .into_response(),
        Ok(None) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "forum_sync_unavailable",
            "论坛同步启用状态尚无有效观测",
        )
        .into_response(),
        Err(error) => map_reader_error(error),
    }
}

/// 复用安全论坛 DTO，不向管理员浏览器暴露凭据、Worker 租约或内部错误详情。
pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Err(response) = authorize(&state, &headers, address, &uri).await {
        return *response;
    }
    let Some(auction_id) = parse_canonical_uuid(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    let Some(reader) = state.forum_reader() else {
        return super::super::storage_unavailable("论坛同步状态暂不可用");
    };
    let thread = match reader.public_forum_thread(auction_id).await {
        Ok(Some(thread)) => thread,
        Ok(None) => {
            return api_error(StatusCode::NOT_FOUND, "not_found", "论坛镜像不存在").into_response();
        }
        Err(error) => return map_reader_error(error),
    };
    if thread.auction_id != auction_id || thread.validate().is_err() {
        return super::super::storage_unavailable("论坛同步状态暂不可用");
    }
    let response = PublicForumResponse {
        auction_id: thread.auction_id.to_string(),
        status: thread.status,
        source_revision: thread.source_revision,
        thread_url: thread.thread_url,
        title: thread.title,
        body: thread.body,
        content_revision: thread.content_revision,
        processed_through_post_number: thread.processed_through_post_number,
        content_synced_at: thread.content_synced_at.map(timestamp),
        content_last_success_at: thread.content_last_success_at.map(timestamp),
        content_last_error_at: thread.content_last_error_at.map(timestamp),
        content_last_error_code: thread.content_last_error_code,
        updated_at: timestamp(thread.updated_at),
    };
    (no_store_headers(), Json(ApiResponse::ok("论坛拍卖帖子同步状态已加载", response)))
        .into_response()
}

fn map_reader_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || {
        if matches!(error, ApplicationError::CapabilityUnavailable(_)) {
            api_error(
                StatusCode::NOT_IMPLEMENTED,
                "feature_not_supported",
                "论坛镜像仅支持 PostgreSQL 完整档",
            )
            .into_response()
        } else {
            super::super::storage_unavailable("论坛同步状态暂不可用")
        }
    };
    let response = body();
    diagnostics::mapped_response("admin_forum", "respond_status_failure", diagnostic_kind, response)
}
