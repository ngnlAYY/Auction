use super::*;

/// 公开 lot 列表。preview 与 live 使用互斥的数组形状，保持旧系统的 JSON 契约。
pub(crate) async fn list_lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
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
        return Err(lot_dependency_unavailable());
    };
    let page_result = reader
        .list_public_lots(auction_id, request)
        .await
        .map_err(|_| lot_dependency_unavailable())?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.validate(auction_id, query.get("category").map(String::as_str)).is_err()
        || page_result.limit != page_size
        || page_result.offset != offset
    {
        return Err(lot_dependency_unavailable());
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
    let lot_id = parse_public_uuid(&lot_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效"))?;
    let Some(reader) = state.lots() else {
        return Err(lot_dependency_unavailable());
    };
    let detail = reader.public_lot(lot_id).await.map_err(|_| lot_dependency_unavailable())?;
    let Some(detail) = detail else {
        return Err(not_found_error());
    };
    if !valid_lot_detail(&detail, lot_id) {
        return Err(lot_dependency_unavailable());
    }
    let response = map_lot_detail(detail);
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

fn lot_dependency_unavailable() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
}
