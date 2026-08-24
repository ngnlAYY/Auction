use std::net::SocketAddr;

use auction_application::{AdminCreateAuctionCommand, RequestHash};
use auction_contracts::{AdminAuctionCreateResponse, AdminCreateAuctionRequest, ApiResponse};
use auction_domain::{AuctionId, CommandId};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_session,
    authentication_error, create_feature_unavailable, internal_error, map_create_error,
    no_store_headers, read_admin_cookie, request_id, valid_admin_csrf_header,
};

/// 创建 Admin draft 拍卖。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = read_admin_cookie(&headers, super::ADMIN_CSRF_COOKIE) else {
        return authentication_error();
    };
    if !valid_admin_csrf_header(&headers, &raw_csrf) {
        return api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "创建拍卖不接受查询参数")
            .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminCreateAuctionRequest = match serde_json::from_slice(&body) {
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
    if input.auction_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    if input.expected_state_version != 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_state_version",
            "创建拍卖必须使用零版本 fence",
        )
        .into_response();
    }
    let request_hash =
        match RequestHash::from_request("admin_auction_create", input.auction_id, &input) {
            Ok(hash) => hash,
            Err(_) => return internal_error(),
        };
    let Some(port) = state.auction_create() else {
        return create_feature_unavailable();
    };
    let result = match port
        .create(AdminCreateAuctionCommand {
            command_id: CommandId::from_uuid(input.command_id),
            auction_id: AuctionId::from_uuid(input.auction_id),
            world_id: input.world_id.clone(),
            sequence: input.sequence,
            preview_scheduled_at: input.preview_scheduled_at,
            scheduled_open_at: input.scheduled_open_at,
            scheduled_end_at: input.scheduled_end_at,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_create_error(error),
    };
    if !result.validate()
        || result.auction.id.as_uuid() != input.auction_id
        || result.auction.world_id != input.world_id
        || result.auction.sequence != input.sequence
        || result.auction.status != "draft"
        || result.auction.state_version != 1
    {
        return internal_error();
    }
    (
        StatusCode::CREATED,
        no_store_headers(),
        Json(ApiResponse::ok(
            "后台拍卖已创建",
            AdminAuctionCreateResponse { auction: super::map_auction(result) },
        )),
    )
        .into_response()
}
