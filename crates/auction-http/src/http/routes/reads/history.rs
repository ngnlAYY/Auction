use super::*;

/// 公开竞价日志。编号页与 keyset cursor 页沿用旧系统互斥的分页字段。
pub(crate) async fn list_bid_log(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicBidLogListResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
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
    let (lot_id, lot_code) = match query.get("lot") {
        None => (None, None),
        Some(lot) if lot.is_empty() => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
        }
        Some(lot) => match parse_public_uuid(lot) {
            Ok(lot_id) => (Some(lot_id), None),
            Err(_) if lot.len() <= 32 && lot.bytes().all(|byte| byte.is_ascii_alphanumeric()) => {
                (None, Some(lot.clone()))
            }
            Err(_) => {
                return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"));
            }
        },
    };
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, None)
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
        lot_code,
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
    let page_size = parse_positive_i64(query.get("page_size"), 50, None)
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
    let page_result =
        reader.list_public_price_history(request).await.map_err(|error| match error {
            ApplicationError::CapabilityUnavailable(_) => api_error(
                StatusCode::NOT_IMPLEMENTED,
                "feature_not_supported",
                "公开价格历史仅支持 PostgreSQL 完整档",
            ),
            _ => api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "公开价格历史暂不可用",
            ),
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
