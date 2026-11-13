//! Admin 凭据导入、变更与删除的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminCredentialDeleteCommand, AdminCredentialImportCommand, AdminCredentialImportSource,
    AdminCredentialKind, AdminCredentialLogin, AdminCredentialRenameCommand, AdminCredentialSecret,
    AdminCredentialStatus, AdminCredentialStatusCommand, AdminCredentialWorld,
};
use auction_contracts::{
    AdminCredentialCreateRequest, AdminCredentialDeleteRequest, AdminCredentialDeleteResponse,
    AdminCredentialMutationRequest, AdminCredentialMutationResponse, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;

use super::{
    AdminApiState, authenticated_admin_mutation, feature_unavailable, invalid_json,
    invalid_request, map_credential, map_error, mutation_boundary, no_store_headers, request_id,
};

/// 导入新的 mail_cookie/forum_cookie。
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
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminCredentialCreateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 无效");
    }
    let kind = if input.kind.is_empty() {
        AdminCredentialKind::MailCookie
    } else {
        match AdminCredentialKind::parse(&input.kind) {
            Some(kind) => kind,
            None => return invalid_request("invalid_credential_kind", "凭据类型无效"),
        }
    };
    let world = match AdminCredentialWorld::parse(&input.world) {
        Some(world) => world,
        None => return invalid_request("invalid_credential_world", "凭据 world 无效"),
    };
    if !auction_application::admin::valid_credential_player_name(kind, input.player_name.as_deref())
    {
        return invalid_request(
            "invalid_credential_player_name",
            "预期玩家名称无效或凭据类型不支持",
        );
    }
    let source = match input.login {
        Some(login) if input.secret.is_empty() => {
            match AdminCredentialLogin::new(login.username, login.password) {
                Ok(login) => AdminCredentialImportSource::ForumLogin(login),
                Err(_) => {
                    return invalid_request("invalid_credential_request", "论坛登录字段无效");
                }
            }
        }
        None => match AdminCredentialSecret::new(input.secret) {
            Ok(secret) => AdminCredentialImportSource::Cookie(secret),
            Err(_) => return invalid_request("invalid_credential_secret", "凭据内容无效"),
        },
        Some(_) => {
            return invalid_request(
                "invalid_credential_request",
                "Cookie 与账号密码必须且只能提供一种",
            );
        }
    };
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port
        .import(AdminCredentialImportCommand {
            command_id: input.command_id,
            kind,
            world,
            label: input.label,
            player_name: input.player_name,
            source,
            actor_id: identity.actor,
            operator_identity: None,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::CREATED,
        no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已导入",
            AdminCredentialMutationResponse {
                credential: map_credential(result),
                restart_required: true,
            },
        )),
    )
        .into_response()
}

/// 修改凭据标签或 active/disabled 状态。
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
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminCredentialMutationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() || input.credential_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 或凭据 ID 无效");
    }
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let credential = match input.action.as_str() {
        "rename" => {
            let Some(label) = input.label else {
                return invalid_request("invalid_credential_label", "凭据标签缺失");
            };
            if input.status.is_some() {
                return invalid_request("invalid_credential_action", "改名命令不接受 status");
            }
            match port
                .rename(AdminCredentialRenameCommand {
                    command_id: input.command_id,
                    credential_id: input.credential_id,
                    expected_state_version: input.expected_state_version,
                    label,
                    actor_id: identity.actor,
                    request_id: request_id(&headers),
                    now: Utc::now(),
                })
                .await
            {
                Ok(result) => result,
                Err(error) => return map_error(error),
            }
        }
        "set_status" => {
            let Some(status) = input.status.as_deref().and_then(AdminCredentialStatus::parse)
            else {
                return invalid_request("invalid_credential_status", "凭据状态无效");
            };
            if input.label.is_some() {
                return invalid_request("invalid_credential_action", "状态命令不接受 label");
            }
            match port
                .set_status(AdminCredentialStatusCommand {
                    command_id: input.command_id,
                    credential_id: input.credential_id,
                    expected_state_version: input.expected_state_version,
                    status,
                    actor_id: identity.actor,
                    request_id: request_id(&headers),
                    now: Utc::now(),
                })
                .await
            {
                Ok(result) => result,
                Err(error) => return map_error(error),
            }
        }
        _ => return invalid_request("invalid_credential_action", "凭据动作无效"),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已更新",
            AdminCredentialMutationResponse {
                credential: map_credential(credential),
                restart_required: true,
            },
        )),
    )
        .into_response()
}

/// 删除一条凭据。
pub(crate) async fn delete(
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
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminCredentialDeleteRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() || input.credential_id.is_nil() {
        return invalid_request("invalid_command_id", "命令 ID 或凭据 ID 无效");
    }
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port
        .delete(AdminCredentialDeleteCommand {
            command_id: input.command_id,
            credential_id: input.credential_id,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "凭据已删除",
            AdminCredentialDeleteResponse {
                credential_id: result.credential_id.to_string(),
                state_version: result.state_version,
                restart_required: true,
            },
        )),
    )
        .into_response()
}
