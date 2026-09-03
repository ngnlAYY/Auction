//! Public 论坛镜像 HTTP 适配器。

use super::{
    PublicApiState, api_error, not_found_error, parse_public_uuid, parse_query, public_timestamp,
};
use auction_application::{ApplicationError, PublicForumThread};
use auction_contracts::{ApiErrorResponse, ApiResponse, PublicForumResponse};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
};

/// 读取单场拍卖的本地论坛镜像状态。
pub(crate) async fn get(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<(HeaderMap, Json<ApiResponse<PublicForumResponse>>), (StatusCode, Json<ApiErrorResponse>)>
{
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    parse_query(&uri, &[])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let Some(reader) = state.forum() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "论坛镜像暂不可用",
        ));
    };
    let thread = reader.public_forum_thread(auction_id).await.map_err(map_reader_error)?;
    let Some(thread) = thread else {
        return Err(not_found_error());
    };
    if thread.auction_id != auction_id || thread.validate().is_err() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "论坛镜像返回数据无效",
        ));
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, Json(ApiResponse::ok("论坛拍卖帖子同步状态已加载", map_thread(thread)))))
}

fn map_thread(thread: PublicForumThread) -> PublicForumResponse {
    PublicForumResponse {
        auction_id: thread.auction_id.to_string(),
        status: thread.status,
        source_revision: thread.source_revision,
        thread_url: thread.thread_url,
        title: thread.title,
        body: thread.body,
        content_revision: thread.content_revision,
        processed_through_post_number: thread.processed_through_post_number,
        content_synced_at: thread.content_synced_at.map(public_timestamp),
        content_last_success_at: thread.content_last_success_at.map(public_timestamp),
        content_last_error_at: thread.content_last_error_at.map(public_timestamp),
        content_last_error_code: thread.content_last_error_code,
        updated_at: public_timestamp(thread.updated_at),
    }
}

fn map_reader_error(error: ApplicationError) -> (StatusCode, Json<ApiErrorResponse>) {
    if matches!(error, ApplicationError::CapabilityUnavailable(_)) {
        api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "论坛镜像仅支持 PostgreSQL 完整档",
        )
    } else {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "论坛镜像暂不可用")
    }
}
