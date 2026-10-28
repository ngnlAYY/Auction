//! Admin 拍卖生命周期命令处理器。

use super::super::*;

pub(crate) async fn admin_preview_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    request: Request,
) -> Response {
    execute_admin_lifecycle(state, address, auction_id, request, AuctionLifecycleAction::Preview)
        .await
}

pub(crate) async fn admin_open_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    request: Request,
) -> Response {
    execute_admin_lifecycle(state, address, auction_id, request, AuctionLifecycleAction::Open).await
}

pub(crate) async fn admin_cancel_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    request: Request,
) -> Response {
    execute_admin_lifecycle(state, address, auction_id, request, AuctionLifecycleAction::Cancel)
        .await
}

pub(crate) async fn admin_final_phase_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    request: Request,
) -> Response {
    execute_admin_lifecycle(state, address, auction_id, request, AuctionLifecycleAction::FinalPhase)
        .await
}

pub(crate) async fn admin_early_settlement_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    request: Request,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        request,
        AuctionLifecycleAction::EarlySettlement,
    )
    .await
}

pub(crate) async fn execute_admin_lifecycle(
    state: AdminApiState,
    address: SocketAddr,
    auction_id: String,
    request: Request,
    action: AuctionLifecycleAction,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let uri = parts.uri;
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "生命周期命令不接受查询参数")
            .into_response();
    }
    let Some(auction_uuid) = parse_canonical_uuid(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    let input: AdminAuctionLifecycleRequest =
        match strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Some(command_uuid) = parse_canonical_uuid(&input.command_id) else {
        return invalid_lifecycle_command(action);
    };
    if input.expected_state_version <= 0 {
        return invalid_lifecycle_command(action);
    }
    let Ok(expected_state_version) = u64::try_from(input.expected_state_version) else {
        return invalid_lifecycle_command(action);
    };
    if matches!(action, AuctionLifecycleAction::Cancel | AuctionLifecycleAction::EarlySettlement) {
        let Some(reason) = input.reason.as_deref() else {
            return invalid_lifecycle_command(action);
        };
        if reason.trim().is_empty() || reason.len() > 1024 || reason.chars().any(char::is_control) {
            return invalid_lifecycle_command(action);
        }
    } else if input.reason.is_some() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
            .into_response();
    }
    let request_hash = match RequestHash::from_request(action.command_kind(), auction_uuid, &input)
    {
        Ok(hash) => hash,
        Err(_) => return internal_error(),
    };
    let Some(lifecycle) = state.lifecycle() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖生命周期暂不可用",
        )
        .into_response();
    };
    let command = AuctionLifecycleCommand {
        command_id: CommandId::from_uuid(command_uuid),
        auction_id: auction_domain::AuctionId::from_uuid(auction_uuid),
        action,
        expected_state_version: Some(expected_state_version),
        reason: input.reason,
        request_hash,
        request_id: request_id(&headers),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match lifecycle.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_admin_lifecycle_error(action, error),
    };
    let Some(auction) = result.auction else {
        return internal_error();
    };
    if result.command_id.as_uuid() != command_uuid
        || result.auction_id.as_uuid() != auction_uuid
        || result.state_version == 0
        || auction.id.as_uuid() != auction_uuid
        || auction.status != result.status
        || auction.state_version != result.state_version
        || auction.updated_at != result.updated_at
    {
        return internal_error();
    }
    let Ok(state_version) = i64::try_from(auction.state_version) else {
        return internal_error();
    };
    let data = AdminAuctionLifecycleResponse {
        auction: AdminAuctionCommandResponse {
            id: auction_uuid.to_string(),
            world_id: auction.world_id,
            sequence: auction.sequence,
            status: admin_auction_status_name(auction.status).to_owned(),
            preview_scheduled_at: timestamp(auction.preview_scheduled_at),
            scheduled_open_at: timestamp(auction.scheduled_open_at),
            preview_opened_at: auction.preview_opened_at.map(timestamp),
            preview_manual_open: auction.preview_manual_open,
            preview_revision: auction.preview_revision,
            opened_at: auction.opened_at.map(timestamp),
            final_phase_at: auction.final_phase_at.map(timestamp),
            settled_at: auction.settled_at.map(timestamp),
            cancelled_at: auction.cancelled_at.map(timestamp),
            cancelled_reason: auction.cancelled_reason,
            cancelled_by: auction.cancelled_by,
            state_version,
            created_at: timestamp(auction.created_at),
            updated_at: timestamp(auction.updated_at),
        },
    };
    let message = match action {
        AuctionLifecycleAction::Preview => "后台拍卖预览已开放",
        AuctionLifecycleAction::Open => "后台拍卖已开拍",
        AuctionLifecycleAction::Cancel => "后台拍卖已取消",
        AuctionLifecycleAction::FinalPhase => "后台拍卖已进入最终阶段",
        AuctionLifecycleAction::EarlySettlement => "后台拍卖已提前进入结算队列",
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok(message, data))).into_response()
}

fn invalid_lifecycle_command(action: AuctionLifecycleAction) -> Response {
    let message = match action {
        AuctionLifecycleAction::Preview => "后台拍卖预览开放参数无效",
        AuctionLifecycleAction::Open => "后台拍卖开拍参数无效",
        AuctionLifecycleAction::Cancel => "后台拍卖取消参数无效",
        AuctionLifecycleAction::FinalPhase => "后台拍卖最终阶段参数无效",
        AuctionLifecycleAction::EarlySettlement => "后台拍卖提前结算参数无效",
    };
    api_error(StatusCode::BAD_REQUEST, "invalid_auction_command", message).into_response()
}
