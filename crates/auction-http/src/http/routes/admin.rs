//! Admin 会话、生命周期、结算与恢复 HTTP 处理器。

use super::*;

pub(crate) const ADMIN_SESSION_COOKIE: &str = "auction_admin_session";
pub(crate) const ADMIN_CSRF_COOKIE: &str = "auction_admin_csrf";
pub(crate) const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

pub(crate) async fn login_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some((actor, secret)) = read_admin_basic_auth(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
            .into_response();
    };
    let now = Utc::now();
    match session.login(actor, secret, now).await {
        Ok(result) => {
            let mut response_headers = no_store_headers();
            let Ok(session_cookie) = admin_cookie(
                ADMIN_SESSION_COOKIE,
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                true,
            ) else {
                return internal_error();
            };
            let Ok(csrf_cookie) = admin_cookie(
                ADMIN_CSRF_COOKIE,
                &result.csrf_token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
                false,
            ) else {
                return internal_error();
            };
            response_headers.append(header::SET_COOKIE, session_cookie);
            response_headers.append(header::SET_COOKIE, csrf_cookie);
            let data = AdminSessionResponse {
                authenticated: true,
                actor: result.identity.actor,
                csrf_token: result.csrf_token,
                expires_at: timestamp(result.identity.expires_at),
                absolute_expires_at: timestamp(result.identity.absolute_expires_at),
            };
            (StatusCode::OK, response_headers, Json(ApiResponse::ok("登录成功", data)))
                .into_response()
        }
        Err(error) => map_admin_session_error(error),
    }
}

pub(crate) async fn current_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let (raw_csrf, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let data = AdminSessionResponse {
        authenticated: true,
        actor: identity.actor,
        csrf_token: raw_csrf,
        expires_at: timestamp(identity.expires_at),
        absolute_expires_at: timestamp(identity.absolute_expires_at),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("会话有效", data))).into_response()
}

pub(crate) async fn logout_admin_session(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
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
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_admin_cookie(&headers, ADMIN_SESSION_COOKIE) else {
        return authentication_error();
    };
    let now = Utc::now();
    if let Err(error) = session.current(raw_token.clone(), raw_csrf, now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    if let Err(error) = session.logout(raw_token, now).await {
        if is_unauthenticated(&error) {
            return authentication_error();
        }
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_admin_cookies(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}

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
        expected_state_version: Some(
            u64::try_from(input.expected_state_version).unwrap_or_default(),
        ),
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

/// 读取 PostgreSQL 完整档的结算报告；Lite 档由应用端口明确返回能力缺失。
pub(crate) async fn admin_settlement_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "结算报告不接受查询参数")
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
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some(reader) = state.settlement_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "结算报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.settlement_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "结算报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "结算报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_settlement_report(&report, auction_uuid) {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("结算报告已加载", map_settlement_report(report))),
    )
        .into_response()
}

/// 读取 PostgreSQL 完整档的 recovery 安全投影；内部 claim/lease 字段永不进入 HTTP。
pub(crate) async fn admin_recovery_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复报告不接受查询参数")
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
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let Some(reader) = state.recovery_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.recovery_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "恢复报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "恢复报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_recovery_report(&report, auction_uuid) {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("恢复报告已加载", map_recovery_report(report))),
    )
        .into_response()
}

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
