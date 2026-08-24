use std::net::SocketAddr;

use auction_application::{AdminUserMutationAction, AdminUserMutationCommand};
use auction_contracts::{
    AdminUserBlacklistRequest, AdminUserMutationResponse, AdminUserUnblacklistRequest, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use uuid::Uuid;

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, common, map_user, request_id,
};

/// 将用户加入黑名单。
pub(crate) async fn blacklist(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let input: AdminUserBlacklistRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    mutate_status(
        state,
        address,
        user_id,
        headers,
        uri,
        input.command_id,
        input.expected_state_version,
        AdminUserMutationAction::Blacklist,
        None,
        None,
        Some(input.public_reason),
    )
    .await
}

/// 将用户移出黑名单。
pub(crate) async fn unblacklist(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let input: AdminUserUnblacklistRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    mutate_status(
        state,
        address,
        user_id,
        headers,
        uri,
        input.command_id,
        input.expected_state_version,
        AdminUserMutationAction::Unblacklist,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn mutate_status(
    state: AdminApiState,
    address: SocketAddr,
    user_id: String,
    headers: HeaderMap,
    uri: Uri,
    command_id: Uuid,
    expected_state_version: i64,
    action: AdminUserMutationAction,
    username: Option<String>,
    game_player_id: Option<String>,
    public_reason: Option<String>,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = common::mutation_boundary(&headers, &uri) {
        return response;
    }
    let Ok(user_id) = Uuid::parse_str(&user_id) else {
        return common::invalid_user_id();
    };
    if user_id.is_nil() || command_id.is_nil() {
        return common::invalid_user_id();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .mutate(AdminUserMutationCommand {
            command_id,
            user_id,
            expected_state_version,
            action,
            username,
            game_player_id,
            public_reason,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return common::map_error(error),
    };
    (
        StatusCode::OK,
        super::super::no_store_headers(),
        Json(ApiResponse::ok(
            if action == AdminUserMutationAction::Blacklist {
                "用户已加入黑名单"
            } else {
                "用户已解除黑名单"
            },
            AdminUserMutationResponse { user: map_user(result.user), generated_key: None },
        )),
    )
        .into_response()
}
