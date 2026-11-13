//! Admin 用户创建、变更与改名回退的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminRenameRevertCommand, AdminUserCreateCommand, AdminUserKeyMode, AdminUserMutationAction,
    AdminUserMutationCommand,
};
use auction_application::mail::MailLoginKey;
use auction_contracts::{
    AdminRenameRevertRequest, AdminUserCreateRequest, AdminUserMutationRequest,
    AdminUserMutationResponse, ApiResponse,
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

/// 创建 Admin 用户。
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
    if let Some(response) = common::mutation_boundary(&uri) {
        return response;
    }
    let mut input: AdminUserCreateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    let Some(command_id) = common::parse_canonical_uuid(&input.command_id) else {
        return common::invalid_command_id();
    };
    let key_mode = match input.key_mode.as_str() {
        "manual" => AdminUserKeyMode::Manual,
        "generated" => AdminUserKeyMode::Generated,
        _ => return common::invalid_user_command(),
    };
    let manual_key = match key_mode {
        AdminUserKeyMode::Manual => {
            let encoded = match input.key.take() {
                Some(value) => value,
                None => return common::invalid_user_command(),
            };
            let result = MailLoginKey::from_admin_manual(&encoded).ok();
            let mut bytes = encoded.into_bytes();
            bytes.fill(0);
            match result {
                Some(key) => Some(key),
                None => return common::invalid_user_command(),
            }
        }
        AdminUserKeyMode::Generated => {
            if input.key.take().is_some() {
                return common::invalid_user_command();
            }
            None
        }
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .create(AdminUserCreateCommand {
            command_id,
            username: input.username,
            forum_account_id: input.forum_account_id,
            key_mode,
            manual_key,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return common::map_error(error),
    };
    let generated_key = result.generated_key.as_ref().map(|key| key.with_encoded(str::to_owned));
    (
        StatusCode::CREATED,
        super::super::no_store_headers(),
        Json(ApiResponse::created(
            "用户已创建",
            AdminUserMutationResponse { user: map_user(result.user), generated_key },
        )),
    )
        .into_response()
}

/// 修改 Admin 用户属性或登录 key 状态。
pub(crate) async fn mutate(
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
    if let Some(response) = common::mutation_boundary(&uri) {
        return response;
    }
    let input: AdminUserMutationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    let Some(command_id) = common::parse_canonical_uuid(&input.command_id) else {
        return common::invalid_command_id();
    };
    let Some(user_id) = common::parse_canonical_uuid(&input.user_id) else {
        return common::invalid_user_id();
    };
    let action = match input.action.as_str() {
        "rename" => AdminUserMutationAction::Rename,
        "set_forum_account_id" => AdminUserMutationAction::SetForumAccountId,
        "revoke_key" => AdminUserMutationAction::RevokeKey,
        "reissue_key" => AdminUserMutationAction::ReissueKey,
        _ => return common::invalid_user_command(),
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .mutate(AdminUserMutationCommand {
            command_id,
            user_id,
            expected_state_version: input.expected_state_version,
            action,
            username: input.username,
            forum_account_id: input.forum_account_id,
            public_reason: None,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return common::map_error(error),
    };
    let generated_key = result.generated_key.as_ref().map(|key| key.with_encoded(str::to_owned));
    (
        StatusCode::OK,
        super::super::no_store_headers(),
        Json(ApiResponse::ok(
            "用户已更新",
            AdminUserMutationResponse { user: map_user(result.user), generated_key },
        )),
    )
        .into_response()
}

/// 回滚一条未失效且仍匹配当前用户名的改名记录。
pub(crate) async fn revert_rename(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(rename_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = common::mutation_boundary(&uri) {
        return response;
    }
    let Some(rename_id) = common::parse_canonical_uuid(&rename_id) else {
        return common::invalid_request("invalid_rename_id", "改名记录 ID 无效");
    };
    let input: AdminRenameRevertRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    let Some(command_id) = common::parse_canonical_uuid(&input.command_id) else {
        return common::invalid_command_id();
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .revert_rename(AdminRenameRevertCommand {
            command_id,
            rename_id,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return common::map_revert_error(error),
    };
    (
        StatusCode::OK,
        super::super::no_store_headers(),
        Json(ApiResponse::ok(
            "用户名已回滚",
            AdminUserMutationResponse { user: map_user(result.user), generated_key: None },
        )),
    )
        .into_response()
}
