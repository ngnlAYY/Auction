use super::*;
use auction_contracts::AuctionSummaryV2Response;

/// 保留 v1 字段闭集，已打开的旧页面无需接受新的摘要字段。
pub(crate) async fn list_auctions(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<AuctionListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    auction_list(state, uri, false, map_auction_summary).await
}

/// v2 在同一分页读取中附带论坛地址，避免按行发起额外 HTTP 或数据库请求。
pub(crate) async fn list_auctions_v2(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<
    Json<ApiResponse<AuctionListResponse<AuctionSummaryV2Response>>>,
    (StatusCode, Json<ApiErrorResponse>),
> {
    auction_list(state, uri, true, map_auction_summary_v2).await
}

/// 两个版本共享筛选、分页、游标和错误处理，只有投影选择与条目映射不同。
async fn auction_list<T>(
    state: PublicApiState,
    uri: Uri,
    include_forum_thread_url: bool,
    map_summary: impl FnMut(PublicAuctionSummary) -> T,
) -> Result<Json<ApiResponse<AuctionListResponse<T>>>, (StatusCode, Json<ApiErrorResponse>)> {
    let mut query = parse_query(&uri, &["world", "status", "page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query.remove("world").filter(|value| !value.is_empty());
    if world_id.as_deref().is_some_and(|world| world.len() > 64 || world != world.trim()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let status = query
        .remove("status")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "settled".to_owned());
    if !matches!(status.as_str(), "live" | "final" | "settled") {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let raw_cursor = query.remove("cursor");
    let cursor = raw_cursor
        .as_deref()
        .map(decode_public_cursor)
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    if cursor.is_some() && page != 1 {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page.checked_sub(1)
            .and_then(|page| page.checked_mul(page_size))
            .filter(|offset| *offset <= 10_000)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };

    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    };
    let result = reader
        .list_public_auctions(PublicAuctionListRequest {
            world_id,
            status,
            limit: page_size,
            offset,
            cursor,
            include_forum_thread_url,
        })
        .await
        .map_err(|_| {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "拍卖目录暂不可用")
        })?;
    if result.limit != page_size || result.offset != offset {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    }

    let response = map_auction_page(result, page, raw_cursor, map_summary);
    Ok(Json(ApiResponse::ok("拍卖列表已加载", response)))
}

pub(crate) async fn current_auction(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<CurrentAuctionResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let mut query = parse_query(&uri, &["world"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query
        .remove("world")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "main".to_owned());
    if world_id.len() > 64 || world_id != world_id.trim() {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }
    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "当前拍卖暂不可用",
        ));
    };
    let auction = reader.current_auction(world_id).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "当前拍卖暂不可用")
    })?;
    Ok(Json(ApiResponse::ok(
        if auction.is_some() { "当前拍卖已加载" } else { "当前没有进行中的拍卖" },
        CurrentAuctionResponse { auction: auction.map(map_current_auction) },
    )))
}
