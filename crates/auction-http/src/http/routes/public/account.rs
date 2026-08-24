use super::*;

/// 读取当前用户自己的普通竞价和代理历史，保留旧接口的独立分页字段。
pub(crate) async fn list_my_bids(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let query = match parse_query(&uri, &["page", "page_size", "bid_page", "proxy_page"]) {
        Ok(query) => query,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let independent = query.contains_key("bid_page") || query.contains_key("proxy_page");
    if independent && query.contains_key("page") {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE,
        Some(MAX_PUBLIC_ACCOUNT_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let bid_page =
        match parse_positive_i64(query.get(if independent { "bid_page" } else { "page" }), 1, None)
        {
            Ok(page) => page,
            Err(_) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        };
    let proxy_page = if independent {
        match parse_positive_i64(query.get("proxy_page"), 1, None) {
            Ok(page) => page,
            Err(_) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        }
    } else {
        bid_page
    };
    let Some(bid_offset) = page_offset(bid_page, page_size) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };
    let Some(proxy_offset) = page_offset(proxy_page, page_size) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };
    let request = PublicMyBidsRequest {
        bids: PublicAccountPageRequest { limit: page_size, offset: bid_offset },
        proxies: PublicAccountPageRequest { limit: page_size, offset: proxy_offset },
    };
    if request.validate().is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    };
    let page = match reader.list_public_user_bids(identity.user_id, request).await {
        Ok(page) => page,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户竞价暂不可用",
            )
            .into_response();
        }
    };
    if !valid_account_page(&page, identity.user_id, &request) {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户竞价暂不可用",
        )
        .into_response();
    }
    let data = PublicMyBidsResponse {
        bids: page.bids.into_iter().map(map_owned_bid).collect(),
        proxies: page.proxies.into_iter().map(map_owned_proxy).collect(),
        bid_total: page.bid_total,
        proxy_total: page.proxy_total,
        page: bid_page,
        bid_page,
        proxy_page,
        page_size,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("账户竞价已加载", data)))
        .into_response()
}

/// 读取当前用户在指定公开 lot 上最新的一份代理授权。
pub(crate) async fn get_my_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权查询不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let Some(lots) = state.lots() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开 lot 目录暂不可用",
        )
        .into_response();
    };
    match lots.public_lot(lot_uuid).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_error().into_response(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "公开 lot 目录暂不可用",
            )
            .into_response();
        }
    }
    let Some(reader) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let authorization = match reader.public_user_proxy(lot_uuid, identity.user_id).await {
        Ok(authorization) => authorization,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    if authorization
        .as_ref()
        .is_some_and(|proxy| proxy.lot_id != lot_uuid || proxy.user_id != identity.user_id)
    {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    }
    let data = PublicMyProxyResponse { authorization: authorization.map(map_owned_proxy) };
    let message = if data.authorization.is_some() {
        "代理授权已加载"
    } else {
        "当前批次没有代理授权"
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok(message, data))).into_response()
}
