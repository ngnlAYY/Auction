//! Admin 用户管理 HTTP 适配器。
//!
//! 这里负责 A28/A30/A31 的会话、CSRF、查询参数和 DTO 映射；版本 fence、key 封装、幂等命令、
//! 黑名单历史与审计由 `auction-application` 和 Users storage owner 负责。生成 key 只通过
//! 本次响应返回，HTTP 适配器不会把它写入日志或错误消息。

use std::net::SocketAddr;

use auction_application::{
    AdminRenameRevertCommand, AdminUserCreateCommand, AdminUserError, AdminUserKeyMode,
    AdminUserListRequest, AdminUserMutationAction, AdminUserMutationCommand, AdminUserStatus,
    ApplicationError, MailLoginKey,
};
use auction_contracts::{
    AdminRenameRevertRequest, AdminUserBlacklistHistoryResponse, AdminUserBlacklistRequest,
    AdminUserCreateRequest, AdminUserListItemResponse, AdminUserListResponse,
    AdminUserMutationRequest, AdminUserMutationResponse, AdminUserPaginationResponse,
    AdminUserResponse, AdminUserUnblacklistRequest, ApiResponse,
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

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 Admin 用户列表。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(&uri, &["name", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return invalid_query(),
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page_size =
        match super::parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
            Ok(value) => value,
            Err(_) => return invalid_query(),
        };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(value) if value <= auction_application::MAX_ADMIN_USER_OFFSET => value,
        _ => return invalid_query(),
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminUserStatus::parse(value) {
            Some(status) => Some(status),
            None => return invalid_query(),
        },
    };
    let request = AdminUserListRequest {
        name: query.get("name").filter(|value| !value.is_empty()).cloned(),
        status,
        limit: page_size,
        offset,
    };
    let Some(port) = state.admin_users() else {
        return feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return super::internal_error();
    }
    let response = AdminUserListResponse {
        users: result.users.into_iter().map(map_list_user).collect(),
        pagination: AdminUserPaginationResponse { page, page_size, total: result.total },
    };
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("用户列表已加载", response)))
        .into_response()
}

/// 创建 Admin 用户。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let mut input: AdminUserCreateRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() {
        return invalid_command_id();
    }
    let key_mode = match input.key_mode.as_str() {
        "manual" => AdminUserKeyMode::Manual,
        "generated" => AdminUserKeyMode::Generated,
        _ => return invalid_request("invalid_key_mode", "登录 key 模式无效"),
    };
    let manual_key = match key_mode {
        AdminUserKeyMode::Manual => {
            let encoded = match input.key.take() {
                Some(value) => value,
                None => return invalid_request("invalid_login_key", "手工登录 key 缺失"),
            };
            let result = MailLoginKey::from_encoded(&encoded).ok();
            let mut bytes = encoded.into_bytes();
            bytes.fill(0);
            match result {
                Some(key) => Some(key),
                None => return invalid_request("invalid_login_key", "登录 key 格式无效"),
            }
        }
        AdminUserKeyMode::Generated => {
            if input.key.take().is_some() {
                return invalid_request("invalid_login_key", "生成 key 模式不接受手工 key");
            }
            None
        }
    };
    let Some(port) = state.admin_users() else {
        return feature_unavailable();
    };
    let result = match port
        .create(AdminUserCreateCommand {
            command_id: input.command_id,
            username: input.username,
            game_player_id: input.game_player_id,
            key_mode,
            manual_key,
            actor_id: identity.actor,
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let generated_key = result.generated_key.as_ref().map(|key| key.with_encoded(str::to_owned));
    (
        StatusCode::CREATED,
        super::no_store_headers(),
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
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminUserMutationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() || input.user_id.is_nil() {
        return invalid_command_id();
    }
    let action = match input.action.as_str() {
        "rename" => AdminUserMutationAction::Rename,
        "set_game_player_id" => AdminUserMutationAction::SetGamePlayerId,
        "revoke_key" => AdminUserMutationAction::RevokeKey,
        "reissue_key" => AdminUserMutationAction::ReissueKey,
        _ => return invalid_request("invalid_user_action", "用户动作无效"),
    };
    let Some(port) = state.admin_users() else {
        return feature_unavailable();
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
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let generated_key = result.generated_key.as_ref().map(|key| key.with_encoded(str::to_owned));
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "用户已更新",
            AdminUserMutationResponse { user: map_user(result.user), generated_key },
        )),
    )
        .into_response()
}

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
        Err(_) => return invalid_json(),
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
        Err(_) => return invalid_json(),
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
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let Ok(user_id) = Uuid::parse_str(&user_id) else {
        return invalid_user_id();
    };
    if user_id.is_nil() || command_id.is_nil() {
        return invalid_user_id();
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let Some(port) = state.admin_users() else {
        return feature_unavailable();
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
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
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

/// 回滚一条未失效且仍匹配当前用户名的改名记录。
pub(crate) async fn revert_rename(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(rename_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    let Ok(rename_id) = Uuid::parse_str(&rename_id) else {
        return invalid_request("invalid_rename_id", "改名记录 ID 无效");
    };
    if rename_id.is_nil() {
        return invalid_request("invalid_rename_id", "改名记录 ID 无效");
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminRenameRevertRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    if input.command_id.is_nil() {
        return invalid_command_id();
    }
    let Some(port) = state.admin_users() else {
        return feature_unavailable();
    };
    let result = match port
        .revert_rename(AdminRenameRevertCommand {
            command_id: input.command_id,
            rename_id,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: None,
            now: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "用户名已回滚",
            AdminUserMutationResponse { user: map_user(result.user), generated_key: None },
        )),
    )
        .into_response()
}

fn map_user(user: auction_application::AdminUser) -> AdminUserResponse {
    AdminUserResponse {
        id: user.id.to_string(),
        username: user.username,
        game_player_id: user.game_player_id,
        status: user.status.as_str().to_owned(),
        key_status: user.key_status.as_str().to_owned(),
        state_version: user.state_version,
        created_at: super::timestamp(user.created_at),
        updated_at: super::timestamp(user.updated_at),
    }
}

fn map_list_user(user: auction_application::AdminUserListItem) -> AdminUserListItemResponse {
    AdminUserListItemResponse {
        user: map_user(user.user),
        blacklist_count: user.blacklist_count,
        blacklist_history: user
            .blacklist_history
            .into_iter()
            .map(|entry| AdminUserBlacklistHistoryResponse {
                public_reason: entry.public_reason,
                blacklisted_at: super::timestamp(entry.blacklisted_at),
            })
            .collect(),
    }
}

fn mutation_boundary(headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "用户命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

fn feature_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "feature_unavailable", "用户管理服务暂不可用")
        .into_response()
}

fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

fn invalid_json() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

fn invalid_command_id() -> Response {
    invalid_request("invalid_command_id", "命令 ID 无效")
}

fn invalid_user_id() -> Response {
    invalid_request("invalid_user_id", "用户 ID 无效")
}

fn invalid_request(code: &'static str, message: &'static str) -> Response {
    super::api_error(StatusCode::BAD_REQUEST, code, message).into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminUser(AdminUserError::InvalidRequest) => {
            invalid_request("invalid_user_request", "用户管理参数无效")
        }
        ApplicationError::AdminUser(AdminUserError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminUser(AdminUserError::Conflict)
        | ApplicationError::AdminUser(AdminUserError::CommandConflict) => super::api_error(
            StatusCode::CONFLICT,
            "command_conflict",
            "用户状态或命令负载与当前记录冲突",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::VersionConflict { .. }) => super::api_error(
            StatusCode::CONFLICT,
            "stale_user_state",
            "用户状态版本已变化，请重新读取后再提交",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::RenameNotRevertable) => super::api_error(
            StatusCode::CONFLICT,
            "rename_not_revertable",
            "该改名记录当前不可回滚",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::KeyUnavailable) => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "key_unavailable",
            "登录 key 服务暂不可用",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::StoredResultCorrupt) => super::internal_error(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "用户管理服务暂不可用",
        )
        .into_response(),
    }
}
