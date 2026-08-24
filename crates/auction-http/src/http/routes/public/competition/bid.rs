//! Public 普通竞价命令 handler。

use super::super::*;

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
