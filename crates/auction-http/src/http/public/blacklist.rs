//! Public 黑名单 HTTP 适配器。

use auction_application::{
    MAX_PUBLIC_BLACKLIST_OFFSET, MAX_PUBLIC_BLACKLIST_PAGE_SIZE, PublicBlacklistEntry,
};
use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicBlacklistEntryResponse, PublicBlacklistResponse,
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
};

use super::{PublicApiState, api_error, parse_positive_i64, parse_query, public_timestamp};

/// 读取当前公开黑名单，保留旧系统的 `users`、页码和总数形状。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicBlacklistResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let query = parse_query(&uri, &["page", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_pagination", "分页参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_BLACKLIST_PAGE_SIZE))
            .map_err(|_| {
                api_error(StatusCode::BAD_REQUEST, "invalid_pagination", "分页参数无效")
            })?;
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_BLACKLIST_OFFSET)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_pagination", "分页参数无效"))?;

    let Some(reader) = state.blacklist() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "黑名单暂不可用",
        ));
    };
    let request = auction_application::PublicBlacklistRequest { limit: page_size, offset };
    let result = reader.list_public_blacklist(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "黑名单暂不可用")
    })?;
    if result.validate().is_err() || result.limit != page_size || result.offset != offset {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "黑名单暂不可用",
        ));
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, Json(ApiResponse::ok("黑名单已加载", map_page(result, page)))))
}

fn map_page(
    page: auction_application::PublicBlacklistPage,
    page_number: i64,
) -> PublicBlacklistResponse {
    PublicBlacklistResponse {
        users: page.entries.into_iter().map(map_entry).collect(),
        total: page.total,
        page: page_number,
        page_size: page.limit,
    }
}

fn map_entry(entry: PublicBlacklistEntry) -> PublicBlacklistEntryResponse {
    PublicBlacklistEntryResponse {
        game_player_id: entry.game_player_id,
        reason: entry.public_reason,
        blacklisted_at: public_timestamp(entry.blacklisted_at),
    }
}
