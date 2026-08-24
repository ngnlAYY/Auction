//! Public 会话、账户、竞价与代理授权 HTTP 处理器。

use super::*;

pub(crate) const PUBLIC_SESSION_COOKIE: &str = "auction_public_session";

pub(crate) async fn login_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let input: PublicLoginRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let now = Utc::now();
    match session.login(input.key.into_bytes(), now).await {
        Ok(result) => {
            let mut response_headers = no_store_headers();
            let Ok(cookie) = public_session_cookie(
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
            ) else {
                return internal_error();
            };
            response_headers.insert(header::SET_COOKIE, cookie);
            let data = PublicLoginResponse {
                user_id: result.identity.user_id.to_string(),
                username: result.identity.username,
                status: "active".to_owned(),
                expires_at: timestamp(result.identity.expires_at),
                absolute_expires_at: timestamp(result.identity.absolute_expires_at),
            };
            (StatusCode::OK, response_headers, Json(ApiResponse::ok("登录成功", data)))
                .into_response()
        }
        Err(error) => map_login_error(error),
    }
}

pub(crate) async fn current_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return current_session_response(&state, None, true);
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => current_session_response(&state, Some(identity), false),
        Err(error) if is_unauthenticated(&error) => current_session_response(&state, None, true),
        Err(_) => session_unavailable(),
    }
}

pub(crate) async fn logout_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return authentication_error();
    };
    let now = Utc::now();
    if let Err(error) = session.current(raw_token.clone(), now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    if session.logout(raw_token, now).await.is_err() {
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}

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

/// Public 普通竞价命令：认证、请求摘要和事务结果都在 HTTP 边界完成稳定映射。
pub(crate) async fn place_bid(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "出价不接受查询参数")
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
    let input: PlaceBidRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let amount = match BidAmount::new(input.amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("place_bid", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let command = auction_application::PlaceBidCommand {
        command_id,
        lot_id,
        bidder_id,
        amount,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.place_bid(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Bid,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(&state, BusinessEvent::Bid, BusinessOutcome::Success, command_started);
    let Some(current_price) = plan.final_lot.current_price else {
        return internal_error();
    };
    let Ok(state_version) = i64::try_from(plan.final_lot.state_version) else {
        return internal_error();
    };
    if plan.final_lot.lot_id != lot_id
        || plan.final_lot.leader_id.is_none()
        || !plan.public_bids.iter().any(|bid| {
            bid.command_id == command_id && bid.lot_id == lot_id && bid.bidder_id == bidder_id
        })
    {
        return internal_error();
    }
    let data = PlaceBidResponse {
        bid: PlaceBidResultResponse {
            lot_id: lot_uuid.to_string(),
            current_price: current_price.value(),
            effective_end_at: timestamp(plan.final_lot.effective_end_at),
            state_version,
            leading: plan.final_lot.leader_id == Some(bidder_id),
        },
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("出价已接受", data))).into_response()
}

/// 创建代理授权命令；命令账本和代理快照提交后返回旧契约要求的完整命令投影。
pub(crate) async fn create_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权不接受查询参数")
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
    let input: CreateProxyAuthorizationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let mode = match input.mode.as_str() {
        "" => None,
        "fast" => Some(ProxyMode::Fast),
        "slow" => Some(ProxyMode::Slow),
        _ => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_proxy_mode", "代理模式无效")
                .into_response();
        }
    };
    let maximum_amount = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("create_proxy", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let command = CreateProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        maximum_amount,
        mode,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.create_proxy(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Proxy,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(
        &state,
        BusinessEvent::Proxy,
        BusinessOutcome::Success,
        command_started,
    );
    let Some(expected_state) = plan.proxy_states.iter().find(|state| {
        state.lot_id == lot_id
            && state.user_id == bidder_id
            && state.creation_command_id == command_id
    }) else {
        return internal_error();
    };
    let Some(account) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let proxy = match account
        .public_user_proxy_by_id(lot_uuid, expected_state.id.as_uuid(), identity.user_id)
        .await
    {
        Ok(Some(proxy)) if valid_command_proxy_identity(&proxy, expected_state) => proxy,
        Ok(_) => return internal_error(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    let Some(data) = map_proxy_command_response(proxy, &plan, expected_state, bidder_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权已创建", data)))
        .into_response()
}

/// 降低代理授权上限命令；命令账本保证重试不会重复追加转换事实，并返回完整命令投影。
pub(crate) async fn lower_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path((lot_id, authorization_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "代理授权不接受查询参数")
            .into_response();
    }
    let Ok(lot_uuid) = Uuid::parse_str(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    let Ok(authorization_uuid) = Uuid::parse_str(&authorization_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_authorization_id", "代理授权 ID 无效")
            .into_response();
    };
    if lot_uuid.is_nil() || authorization_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    }
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let input: LowerProxyAuthorizationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let new_maximum = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
                .into_response();
        }
        Err(_) => return internal_error(),
    };
    let request_hash = match RequestHash::from_request("lower_proxy", lot_uuid, &input) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "竞价服务暂不可用",
        )
        .into_response();
    };
    let command_id = CommandId::from_uuid(input.command_id);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let authorization_id = ProxyAuthorizationId::from_uuid(authorization_uuid);
    let command = LowerProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        authorization_id,
        new_maximum,
        request_hash,
        request_id: request_id(&headers),
    };
    let command_started = Instant::now();
    let plan = match commands.lower_proxy(command).await {
        Ok(plan) => plan,
        Err(error) => {
            observe_public_business(
                &state,
                BusinessEvent::Proxy,
                business_outcome_for_competition_error(&error),
                command_started,
            );
            return map_competition_error(error);
        }
    };
    observe_public_business(
        &state,
        BusinessEvent::Proxy,
        BusinessOutcome::Success,
        command_started,
    );
    let Some(expected_state) = plan.proxy_states.iter().find(|state| {
        state.id == authorization_id && state.lot_id == lot_id && state.user_id == bidder_id
    }) else {
        return internal_error();
    };
    if plan.final_lot.lot_id != lot_id
        || !plan.proxy_transitions.iter().any(|transition| {
            transition.authorization_id == authorization_id && transition.command_id == command_id
        })
    {
        return internal_error();
    }
    let Some(account) = state.account() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "账户代理暂不可用",
        )
        .into_response();
    };
    let proxy = match account
        .public_user_proxy_by_id(lot_uuid, expected_state.id.as_uuid(), identity.user_id)
        .await
    {
        Ok(Some(proxy)) if valid_command_proxy_identity(&proxy, expected_state) => proxy,
        Ok(_) => return internal_error(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "账户代理暂不可用",
            )
            .into_response();
        }
    };
    let Some(data) = map_proxy_command_response(proxy, &plan, expected_state, bidder_id) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权上限已降低", data)))
        .into_response()
}
