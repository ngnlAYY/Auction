//! Public 普通竞价命令 handler。

use super::super::*;

/// Public 普通竞价命令：认证、请求摘要和事务结果都在 HTTP 边界完成稳定映射。
pub(crate) async fn place_bid(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let (identity, lot_uuid) =
        match super::command_entry(&state, &headers, address, &parts.uri, &lot_id).await {
            Ok(entry) => entry,
            Err(response) => return *response,
        };

    let input: PlaceBidRequest =
        match strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_uuid) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    let amount = match BidAmount::new(input.amount) {
        Ok(amount) => amount,
        Err(DomainError::InvalidAmount { .. }) if input.amount <= 0 => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效")
                .into_response();
        }
        // 旧 v1 只在 HTTP 层拒绝非正数；正数的网格错误由命令 owner 返回通用依赖外壳。
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "服务暂时不可用",
            )
            .into_response();
        }
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.commands() else {
        return competition_unavailable();
    };
    let command_id = CommandId::from_uuid(command_uuid);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let request_hash = match auction_application::PlaceBidCommand::legacy_request_hash(
        lot_id, bidder_id, amount,
    ) {
        Ok(request_hash) => request_hash,
        Err(_) => return internal_error(),
    };
    let command = auction_application::PlaceBidCommand {
        command_id,
        lot_id,
        bidder_id,
        amount,
        request_hash,
        request_id: request_id(&headers),
    };
    let outcome = match super::observed_command(
        &state,
        BusinessEvent::Bid,
        commands.place_bid(command),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(response) => return *response,
    };
    if outcome.proxy.is_some() {
        return internal_error();
    }
    let effective_end_at = outcome.effective_end_at;
    let plan = outcome.plan;
    let Some(current_price) = plan.final_lot.current_price else {
        return internal_error();
    };
    let Ok(state_version) = i64::try_from(plan.final_lot.state_version) else {
        return internal_error();
    };
    if effective_end_at.instant() != plan.final_lot.effective_end_at
        || plan.final_lot.lot_id != lot_id
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
            effective_end_at: effective_end_at.wire().to_owned(),
            state_version,
            leading: plan.final_lot.leader_id == Some(bidder_id),
        },
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("出价已接受", data))).into_response()
}
