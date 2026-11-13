//! Admin 用户黑名单动作的处理器。

use std::net::SocketAddr;

use auction_application::admin::{AdminUserMutationAction, AdminUserMutationCommand};
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

use super::{AdminApiState, authenticated_admin_mutation, common, map_user, request_id};

/// 将用户加入黑名单。
pub(crate) async fn blacklist(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    mutate_status(state, address, user_id, headers, uri, body, AdminUserMutationAction::Blacklist)
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
    mutate_status(state, address, user_id, headers, uri, body, AdminUserMutationAction::Unblacklist)
        .await
}

async fn mutate_status(
    state: AdminApiState,
    address: SocketAddr,
    user_id: String,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
    action: AdminUserMutationAction,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = common::mutation_boundary(&uri) {
        return response;
    }
    let Some(user_id) = common::parse_canonical_uuid(&user_id) else {
        return common::invalid_user_id();
    };
    let (raw_command_id, expected_state_version, public_reason) = match action {
        AdminUserMutationAction::Blacklist => {
            let input: AdminUserBlacklistRequest = match serde_json::from_slice(&body) {
                Ok(input) => input,
                Err(_) => return common::invalid_json(),
            };
            (input.command_id, input.expected_state_version, Some(input.public_reason))
        }
        AdminUserMutationAction::Unblacklist => {
            let input: AdminUserUnblacklistRequest = match serde_json::from_slice(&body) {
                Ok(input) => input,
                Err(_) => return common::invalid_json(),
            };
            (input.command_id, input.expected_state_version, None)
        }
        _ => return common::invalid_user_command(),
    };
    let Some(command_id) = common::parse_canonical_uuid(&raw_command_id) else {
        return common::invalid_command_id();
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
            username: None,
            forum_account_id: None,
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
            "用户黑名单状态已更新",
            AdminUserMutationResponse { user: map_user(result.user), generated_key: None },
        )),
    )
        .into_response()
}
