use std::net::SocketAddr;

use auction_application::{
    AdminRenameRevertCommand, AdminUserCreateCommand, AdminUserKeyMode, AdminUserMutationAction,
    AdminUserMutationCommand, MailLoginKey,
};
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
use uuid::Uuid;

use super::{
    AdminApiState, admin_session_guard, authenticated_admin_session, common, map_user, request_id,
};

/// 创建 Admin 用户。
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
    if let Some(response) = common::mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let mut input: AdminUserCreateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    if input.command_id.is_nil() {
        return common::invalid_command_id();
    }
    let key_mode = match input.key_mode.as_str() {
        "manual" => AdminUserKeyMode::Manual,
        "generated" => AdminUserKeyMode::Generated,
        _ => return common::invalid_request("invalid_key_mode", "登录 key 模式无效"),
    };
    let manual_key = match key_mode {
        AdminUserKeyMode::Manual => {
            let encoded = match input.key.take() {
                Some(value) => value,
                None => return common::invalid_request("invalid_login_key", "手工登录 key 缺失"),
            };
            let result = MailLoginKey::from_encoded(&encoded).ok();
            let mut bytes = encoded.into_bytes();
            bytes.fill(0);
            match result {
                Some(key) => Some(key),
                None => return common::invalid_request("invalid_login_key", "登录 key 格式无效"),
            }
        }
        AdminUserKeyMode::Generated => {
            if input.key.take().is_some() {
                return common::invalid_request("invalid_login_key", "生成 key 模式不接受手工 key");
            }
            None
        }
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .create(AdminUserCreateCommand {
            command_id: input.command_id,
            username: input.username,
            game_player_id: input.game_player_id,
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
        Json(ApiResponse::ok(
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
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = common::mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminUserMutationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    if input.command_id.is_nil() || input.user_id.is_nil() {
        return common::invalid_command_id();
    }
    let action = match input.action.as_str() {
        "rename" => AdminUserMutationAction::Rename,
        "set_game_player_id" => AdminUserMutationAction::SetGamePlayerId,
        "revoke_key" => AdminUserMutationAction::RevokeKey,
        "reissue_key" => AdminUserMutationAction::ReissueKey,
        _ => return common::invalid_request("invalid_user_action", "用户动作无效"),
    };
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .mutate(AdminUserMutationCommand {
            command_id: input.command_id,
            user_id: input.user_id,
            expected_state_version: input.expected_state_version,
            action,
            username: input.username,
            game_player_id: input.game_player_id,
            public_reason: input.public_reason,
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
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = common::mutation_boundary(&headers, &uri) {
        return response;
    }
    let Ok(rename_id) = Uuid::parse_str(&rename_id) else {
        return common::invalid_request("invalid_rename_id", "改名记录 ID 无效");
    };
    if rename_id.is_nil() {
        return common::invalid_request("invalid_rename_id", "改名记录 ID 无效");
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminRenameRevertRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return common::invalid_json(),
    };
    if input.command_id.is_nil() {
        return common::invalid_command_id();
    }
    let Some(port) = state.admin_users() else {
        return common::feature_unavailable();
    };
    let result = match port
        .revert_rename(AdminRenameRevertCommand {
            command_id: input.command_id,
            rename_id,
            expected_state_version: input.expected_state_version,
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
            "用户名已回滚",
            AdminUserMutationResponse { user: map_user(result.user), generated_key: None },
        )),
    )
        .into_response()
}
