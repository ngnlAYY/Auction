//! Public 只读查询处理器。

use super::*;

pub(crate) async fn list_worlds(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<WorldsResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "世界目录不接受查询参数"));
    }

    let Some(reader) = state.worlds() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "世界目录暂不可用",
        ));
    };
    let worlds = reader.list_worlds().await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "世界目录暂不可用")
    })?;
    let worlds = worlds.into_iter().map(map_world).collect();
    Ok(Json(ApiResponse::ok("世界状态已加载", WorldsResponse { worlds })))
}

pub(crate) fn map_world(world: WorldSnapshot) -> (String, WorldStatusResponse) {
    (world.id, WorldStatusResponse { enabled: world.enabled })
}

pub(crate) async fn list_auctions(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<AuctionListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["world", "status", "page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query.get("world").filter(|value| !value.is_empty()).cloned();
    if world_id.as_deref().is_some_and(|world| world.len() > 64 || world != world.trim()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let status = query
        .get("status")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("settled");
    if !matches!(status, "live" | "final" | "settled") {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
    }

    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, Some(200))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor = query
        .get("cursor")
        .map(|raw| decode_public_cursor(raw))
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
            status: status.to_owned(),
            limit: page_size,
            offset,
            cursor: cursor.clone(),
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

    let response = map_auction_page(result, page, query.get("cursor").cloned());
    Ok(Json(ApiResponse::ok("拍卖列表已加载", response)))
}

/// 公开竞价日志。编号页与 keyset cursor 页沿用旧系统互斥的分页字段。
pub(crate) async fn list_bid_log(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicBidLogListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = Uuid::parse_str(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(&uri, &["world", "lot", "page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = match query.get("world") {
        None => None,
        Some(world) if world.is_empty() || world.len() > 64 || world != world.trim() => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
        }
        Some(world) => Some(world.clone()),
    };
    let lot_id = match query.get("lot") {
        None => None,
        Some(lot) if lot.is_empty() => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
        }
        Some(lot) => Some(
            Uuid::parse_str(lot)
                .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?,
        ),
    };
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 20, Some(MAX_PUBLIC_BID_LOG_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor_raw = query.get("cursor").cloned();
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_bid_log_cursor)
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
            .filter(|offset| *offset <= MAX_PUBLIC_BID_LOG_OFFSET)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };
    let request = PublicBidLogListRequest {
        auction_id,
        world_id,
        lot_id,
        limit: page_size,
        offset,
        cursor: cursor.clone(),
    };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.bid_log() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    };
    let page_result = reader.list_public_bid_log(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开竞价日志暂不可用")
    })?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.entries.iter().any(|entry| {
            entry.auction_id != auction_id || lot_id.is_some_and(|lot_id| entry.lot_id != lot_id)
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    }
    let response = map_bid_log_page(page_result, page, cursor_raw);
    Ok(Json(ApiResponse::ok("竞价日志已加载", response)))
}

/// 公开成交价格历史。过滤器和分页边界保持旧 Public 契约，cursor 页不计算统计聚合。
pub(crate) async fn list_price_history(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicPriceHistoryListResponse>>, (StatusCode, Json<ApiErrorResponse>)>
{
    let query = parse_query(
        &uri,
        &[
            "name",
            "world",
            "from",
            "to",
            "min_price",
            "max_price",
            "buyer",
            "seller",
            "page",
            "page_size",
            "cursor",
        ],
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let name_terms = parse_history_name_terms(query.get("name"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = parse_optional_history_text(query.get("world"), 64)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let closed_from = parse_history_date(query.get("from"), false)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let closed_before = parse_history_date(query.get("to"), true)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let minimum_price = parse_history_price(query.get("min_price"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let maximum_price = parse_history_price(query.get("max_price"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let buyer = parse_optional_history_text(query.get("buyer"), 256)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let seller = parse_optional_history_text(query.get("seller"), 256)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let cursor_raw = query.get("cursor").cloned();
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_price_history_cursor)
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
            .filter(|offset| *offset <= MAX_PUBLIC_PRICE_HISTORY_OFFSET)
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?
    };
    let request = PublicPriceHistoryListRequest {
        name_terms,
        world_id,
        closed_from,
        closed_before,
        minimum_price,
        maximum_price,
        buyer,
        seller,
        limit: page_size,
        offset,
        cursor,
    };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.price_history() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    };
    let page_result = reader.list_public_price_history(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开价格历史暂不可用")
    })?;
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_some() != page_result.stats.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.entries.iter().any(|entry| {
            entry.lot_id.is_nil()
                || entry.auction_id.is_nil()
                || entry.world_id.is_empty()
                || entry.equipment_name.is_empty()
                || entry.equipment_category.is_empty()
                || entry.buyer_name.is_empty()
                || entry.seller_name.is_empty()
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    }
    let response = map_price_history_page(page_result, page, cursor_raw);
    Ok(Json(ApiResponse::ok("价格历史已加载", response)))
}

/// 公开 lot 列表。preview 与 live 使用互斥的数组形状，保持旧系统的 JSON 契约。
pub(crate) async fn list_lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = Uuid::parse_str(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(&uri, &["category", "page", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, Some(MAX_PUBLIC_LOT_PAGE_SIZE))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_LOT_OFFSET)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let category = query.get("category").cloned();
    let request = PublicLotListRequest { category, limit: page_size, offset };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.lots() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    };
    let page_result = reader.list_public_lots(auction_id, request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开 lot 目录暂不可用")
    })?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.projection.auction_id != auction_id
        || page_result.limit != page_size
        || page_result.offset != offset
        || page_result.category_counts.len() != 8
        || page_result.projection.projection_kind == "preview" && !page_result.live.is_empty()
        || page_result.projection.projection_kind != "preview" && !page_result.preview.is_empty()
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    }
    let response = map_lot_page(page_result, page);
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

/// 公开 lot 详情只返回仍属于当前公开 projection 的对象。
pub(crate) async fn lot_detail(
    State(state): State<PublicApiState>,
    Path(lot_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "lot 详情不接受查询参数"));
    }
    let lot_id = Uuid::parse_str(&lot_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效"))?;
    let Some(reader) = state.lots() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    };
    let detail = reader.public_lot(lot_id).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开 lot 目录暂不可用")
    })?;
    let Some(detail) = detail else {
        return Err(not_found_error());
    };
    if !valid_lot_detail(&detail, lot_id) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        ));
    }
    let response = map_lot_detail(detail);
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

pub(crate) async fn current_auction(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<CurrentAuctionResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["world"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let world_id = query
        .get("world")
        .filter(|value| !value.is_empty())
        .cloned()
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
