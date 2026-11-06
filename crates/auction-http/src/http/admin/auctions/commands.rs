use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::admin::AdminCreateAuctionCommand;
use auction_application::core::RequestHash;
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
    AdminApiState, api_error, authenticated_admin_mutation, create_feature_unavailable,
    internal_error, invalid_create_command, map_create_error, no_store_headers,
    parse_canonical_uuid, request_id,
};

/// 创建 Admin draft 拍卖。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "创建拍卖不接受查询参数")
            .into_response();
    }
    let input: AdminCreateAuctionRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(command_id) = parse_canonical_uuid(&input.command_id) else {
        return invalid_create_command();
    };
    let Some(auction_id) = parse_canonical_uuid(&input.auction_id) else {
        return invalid_create_command();
    };
    let (Some(preview_scheduled_at), Some(scheduled_open_at), Some(scheduled_end_at)) =
        (input.preview_scheduled_at, input.scheduled_open_at, input.scheduled_end_at)
    else {
        return invalid_create_command();
    };
    if input.expected_state_version != 0 {
        return invalid_create_command();
    }
    let request_hash = match RequestHash::from_request("admin_auction_create", auction_id, &input) {
        Ok(hash) => hash,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_auctions",
                "hash_create",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(port) = state.auction_create() else {
        return create_feature_unavailable();
    };
    let result = match port
        .create(AdminCreateAuctionCommand {
            command_id: CommandId::from_uuid(command_id),
            auction_id: AuctionId::from_uuid(auction_id),
            world_id: input.world_id.clone(),
            sequence: input.sequence,
            preview_scheduled_at,
            scheduled_open_at,
            scheduled_end_at,
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
        || result.auction.id.as_uuid() != auction_id
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
        Json(ApiResponse::created(
            "后台拍卖已创建",
            AdminAuctionCreateResponse { auction: super::map_auction_command(result) },
        )),
    )
        .into_response()
}
