//! Admin recovery case 的人工确认处理器。

use super::super::*;
use super::ADMIN_CSRF_COOKIE;

/// 以版本 fence 确认一个已解决 custody 证据的 recovery case。
pub(crate) async fn admin_confirm_recovery(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(recovery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
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
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复确认不接受查询参数")
            .into_response();
    }
    let Ok(recovery_uuid) = Uuid::parse_str(&recovery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_id", "恢复 ID 无效")
            .into_response();
    };
    if recovery_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_id", "恢复 ID 无效")
            .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminRecoveryConfirmationRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() || input.expected_state_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    }
    let Some(action) = RecoveryConfirmationAction::parse(&input.action) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    };
    let request_hash = match RequestHash::from_request("confirm_recovery", recovery_uuid, &input) {
        Ok(hash) => hash,
        Err(_) => return internal_error(),
    };
    let Some(commands) = state.recovery_commands() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let command = RecoveryConfirmationCommand {
        command_id: CommandId::from_uuid(input.command_id),
        recovery_id: SettlementId::from_uuid(recovery_uuid),
        expected_state_version: input.expected_state_version,
        action,
        reason: input.reason,
        request_hash,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match commands.confirm_recovery(command).await {
        Ok(result) => result,
        Err(error) => return map_recovery_command_error(error),
    };
    if result.recovery_id.as_uuid() != recovery_uuid
        || result.state_version <= input.expected_state_version
    {
        return internal_error();
    }
    let data = AdminRecoveryConfirmationResponse { recovery: map_recovery_confirmation(result) };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("恢复确认已提交", data)))
        .into_response()
}
