//! Admin 拍卖生命周期命令处理器。

use super::super::*;
use super::ADMIN_CSRF_COOKIE;

pub(crate) async fn admin_preview_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Preview,
    )
    .await
}

pub(crate) async fn admin_open_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Open,
    )
    .await
}

pub(crate) async fn admin_cancel_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::Cancel,
    )
    .await
}

pub(crate) async fn admin_final_phase_auction(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    execute_admin_lifecycle(
        state,
        address,
        auction_id,
        headers,
        uri,
        body,
        AuctionLifecycleAction::FinalPhase,
    )
    .await
}

pub(crate) async fn execute_admin_lifecycle(
    state: AdminApiState,
    address: SocketAddr,
    auction_id: String,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    action: AuctionLifecycleAction,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "生命周期命令不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminAuctionLifecycleRequest = match serde_json::from_slice(&body) {
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
    if input.expected_state_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_state_version", "拍卖状态版本无效")
            .into_response();
    }
    let Ok(expected_state_version) = u64::try_from(input.expected_state_version) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_state_version", "拍卖状态版本无效")
            .into_response();
    };
    if action == AuctionLifecycleAction::Cancel {
        let Some(reason) = input.reason.as_deref() else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_cancel_reason",
                "取消拍卖必须提供原因",
            )
            .into_response();
        };
        if reason.trim().is_empty() || reason.len() > 1024 || reason.chars().any(char::is_control) {
            return api_error(StatusCode::BAD_REQUEST, "invalid_cancel_reason", "取消原因无效")
                .into_response();
        }
    } else if input.reason.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_lifecycle_reason",
            "非取消生命周期命令不能携带原因",
        )
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
        command_id: CommandId::from_uuid(input.command_id),
        auction_id: auction_domain::AuctionId::from_uuid(auction_uuid),
        action,
        expected_state_version: Some(expected_state_version),
        cancel_reason: input.reason,
        request_hash,
        request_id: request_id(&headers),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match lifecycle.execute(command).await {
        Ok(result) => result,
        Err(error) => return map_admin_lifecycle_error(error),
    };
    if result.command_id.as_uuid() != input.command_id
        || result.auction_id.as_uuid() != auction_uuid
        || result.state_version == 0
    {
        return internal_error();
    }
    let data = AdminAuctionLifecycleResponse {
        command_id: input.command_id,
        auction_id: auction_uuid.to_string(),
        status: admin_auction_status_name(result.status).to_owned(),
        state_version: result.state_version,
        updated_at: timestamp(result.updated_at),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("拍卖生命周期已更新", data)))
        .into_response()
}
