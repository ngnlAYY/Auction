//! HTTP 会话校验、Cookie、认证错误和业务错误映射。

use super::*;

pub(crate) fn session_guard(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else {
        state.security().allow_mutation(address.ip())
    };
    let retry_after = retry_after?;
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response_headers.insert(header::RETRY_AFTER, value);
    }
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            response_headers,
            Json(ApiErrorResponse {
                code: "rate_limited".to_owned(),
                message: "请求过于频繁".to_owned(),
                data: None,
                request_id: None,
            }),
        )
            .into_response(),
    )
}

pub(crate) fn admin_session_guard(
    state: &AdminApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else {
        state.security().allow_mutation(address.ip())
    };
    let retry_after = retry_after?;
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response_headers.insert(header::RETRY_AFTER, value);
    }
    Some(
        (
            StatusCode::TOO_MANY_REQUESTS,
            response_headers,
            Json(ApiErrorResponse {
                code: "rate_limited".to_owned(),
                message: "请求过于频繁".to_owned(),
                data: None,
                request_id: None,
            }),
        )
            .into_response(),
    )
}

pub(crate) async fn authenticated_admin_session(
    state: &AdminApiState,
    headers: &HeaderMap,
) -> Result<(String, AdminSessionIdentity), Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(session_unavailable()));
    };
    let Some(raw_token) = read_admin_cookie(headers, ADMIN_SESSION_COOKIE) else {
        return Err(Box::new(authentication_error()));
    };
    let Some(raw_csrf) = read_admin_cookie(headers, ADMIN_CSRF_COOKIE) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, raw_csrf.clone(), Utc::now()).await {
        Ok(identity) => Ok((raw_csrf, identity)),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(_) => Err(Box::new(session_unavailable())),
    }
}

pub(crate) fn current_session_response(
    state: &PublicApiState,
    identity: Option<PublicSessionIdentity>,
    clear_cookie: bool,
) -> Response {
    let mut response_headers = no_store_headers();
    if clear_cookie {
        append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    }
    let data = identity.map(|identity| PublicSessionCurrentResponse {
        authenticated: true,
        user_id: identity.user_id.to_string(),
        username: identity.username,
        expires_at: timestamp(identity.expires_at),
        absolute_expires_at: timestamp(identity.absolute_expires_at),
    });
    let message = if data.is_some() { "会话有效" } else { "当前未登录" };
    (StatusCode::OK, response_headers, Json(ApiResponse::ok(message, data))).into_response()
}

pub(crate) fn map_login_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidLoginKey) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_key", "登录凭据格式无效").into_response()
        }
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::AccountInactive { public_reason }) => {
            let Some(public_reason) = public_reason else {
                return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                    .into_response();
            };
            (
                StatusCode::FORBIDDEN,
                Json(ApiErrorResponseWithData {
                    code: "account_blacklisted".to_owned(),
                    message: "账号已被封禁".to_owned(),
                    data: PublicAccountBlockedResponse { public_reason },
                    request_id: None,
                }),
            )
                .into_response()
        }
        ApplicationError::Session(SessionError::SessionLimit) => {
            api_error(StatusCode::CONFLICT, "session_limit", "并发会话数量已达上限").into_response()
        }
        _ => session_unavailable(),
    }
}

pub(crate) fn map_admin_session_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::Unauthenticated) => authentication_error(),
        ApplicationError::Session(SessionError::InvalidConfiguration)
        | ApplicationError::Session(SessionError::TokenGeneration) => session_unavailable(),
        _ => session_unavailable(),
    }
}

pub(crate) fn map_admin_lifecycle_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Domain(DomainError::InvalidTransition { .. })
        | ApplicationError::Domain(DomainError::InvalidSchedule(_))
        | ApplicationError::Domain(DomainError::CancellationCutoff) => {
            api_error(StatusCode::CONFLICT, "invalid_lifecycle", "拍卖当前不允许该生命周期操作")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            api_error(
                StatusCode::CONFLICT,
                "state_version_conflict",
                "拍卖状态已变化，请刷新后重试",
            )
            .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::Serialization(_) => internal_error(),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖生命周期暂不可用",
        )
        .into_response(),
    }
}

pub(crate) fn is_unauthenticated(error: &ApplicationError) -> bool {
    matches!(error, ApplicationError::Session(SessionError::Unauthenticated))
}

pub(crate) fn admin_auction_status_name(status: auction_domain::AuctionStatus) -> &'static str {
    match status {
        auction_domain::AuctionStatus::Draft => "draft",
        auction_domain::AuctionStatus::Preview => "preview",
        auction_domain::AuctionStatus::Live => "live",
        auction_domain::AuctionStatus::Final => "final",
        auction_domain::AuctionStatus::Settling => "settling",
        auction_domain::AuctionStatus::Settled => "settled",
        auction_domain::AuctionStatus::Cancelled => "cancelled",
    }
}

pub(crate) async fn authenticated_public_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
) -> Result<PublicSessionIdentity, Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(session_unavailable()));
    };
    let Some(raw_token) = read_public_session_cookie(headers) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => Ok(identity),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(_) => Err(Box::new(session_unavailable())),
    }
}

pub(crate) fn observe_public_business(
    state: &PublicApiState,
    event: BusinessEvent,
    outcome: BusinessOutcome,
    started: Instant,
) {
    let Some(metrics) = state.metrics() else {
        return;
    };
    let recorder = metrics.business();
    recorder.observe(event, outcome);
    recorder.observe_duration(event, started.elapsed());
}

pub(crate) fn business_outcome_for_competition_error(error: &ApplicationError) -> BusinessOutcome {
    match error {
        ApplicationError::Domain(_)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::NotFound
            | CompetitionTransactionError::Conflict
            | CompetitionTransactionError::StaleState
            | CompetitionTransactionError::InvalidState
            | CompetitionTransactionError::CommandLedger(CommandLedgerError::Conflict),
        ) => BusinessOutcome::Rejected,
        _ => BusinessOutcome::Error,
    }
}

pub(crate) fn map_competition_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidAmount { .. }) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效").into_response()
        }
        ApplicationError::Domain(DomainError::BidTooLow { .. }) => {
            api_error(StatusCode::CONFLICT, "bid_too_low", "出价未达到当前最低价").into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyMaximum) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_maximum", "代理上限当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyDisabled) => {
            api_error(StatusCode::CONFLICT, "proxy_disabled", "当前拍卖未启用代理竞价")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyOutbidLimit) => {
            api_error(StatusCode::CONFLICT, "proxy_outbid_limit", "代理重复进场次数已用尽")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxySlowFinal) => {
            api_error(StatusCode::CONFLICT, "proxy_slow_final", "最终阶段不允许使用慢速代理")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyTransition { .. }) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_state", "代理授权当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::AuctionClosed)
        | ApplicationError::Domain(DomainError::InvalidBidState(_)) => {
            api_error(StatusCode::CONFLICT, "auction_not_open", "当前拍卖不接受竞价")
                .into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::Serialization(_) => internal_error(),
        ApplicationError::CapabilityUnavailable(_)
        | ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::Domain(_)
        | ApplicationError::Repository(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "竞价服务暂不可用")
                .into_response()
        }
    }
}

pub(crate) fn map_recovery_command_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "恢复记录不存在").into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::StaleState) => {
            api_error(StatusCode::CONFLICT, "stale_recovery_state", "恢复状态版本已变化")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable) => {
            api_error(StatusCode::CONFLICT, "recovery_not_confirmable", "恢复状态不允许当前确认")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::EvidenceUnresolved) => {
            api_error(StatusCode::CONFLICT, "recovery_evidence_unresolved", "恢复证据尚未解决")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response(),
        ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::Serialization(_)
        | ApplicationError::Repository(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::Domain(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "恢复确认服务暂不可用",
        )
        .into_response(),
    }
}

pub(crate) fn authentication_error() -> Response {
    api_error(StatusCode::UNAUTHORIZED, "unauthenticated", "会话无效或已过期").into_response()
}

pub(crate) fn session_unavailable() -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "会话服务暂不可用")
        .into_response()
}

pub(crate) fn internal_error() -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "服务内部错误").into_response()
}

pub(crate) fn no_store_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers
}

pub(crate) fn read_public_session_cookie(headers: &HeaderMap) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for pair in value.split(';') {
            let (name, cookie_value) = pair.trim().split_once('=')?;
            if name != PUBLIC_SESSION_COOKIE {
                continue;
            }
            if found.is_some() || cookie_value.is_empty() {
                return None;
            }
            found = Some(cookie_value.to_owned());
        }
    }
    found
}

pub(crate) fn read_admin_basic_auth(headers: &HeaderMap) -> Option<(String, Vec<u8>)> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") || encoded.is_empty() {
        return None;
    }
    let decoded = STANDARD.decode(encoded).ok()?;
    let separator = decoded.iter().position(|byte| *byte == b':')?;
    let actor = std::str::from_utf8(&decoded[..separator]).ok()?.to_owned();
    let secret = decoded.get(separator + 1..)?.to_vec();
    Some((actor, secret))
}

pub(crate) fn read_admin_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let value = value.to_str().ok()?;
        for pair in value.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (cookie_name, cookie_value) = pair.split_once('=')?;
            if cookie_name != name {
                continue;
            }
            if found.is_some() || cookie_value.is_empty() {
                return None;
            }
            found = Some(cookie_value.to_owned());
        }
    }
    found
}

pub(crate) fn valid_admin_csrf_header(headers: &HeaderMap, cookie: &str) -> bool {
    let mut values = headers.get_all(ADMIN_CSRF_HEADER).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    values.next().is_none() && constant_time_equal(cookie, value)
}

pub(crate) fn constant_time_equal(left: &str, right: &str) -> bool {
    let left_hash = Sha256::digest(left.as_bytes());
    let right_hash = Sha256::digest(right.as_bytes());
    left_hash.as_slice().ct_eq(right_hash.as_slice()).unwrap_u8() == 1
}

pub(crate) fn admin_cookie(
    name: &str,
    token: &str,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    secure: bool,
    http_only: bool,
) -> Result<HeaderValue, ()> {
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{name}={token}; Path=/; Expires={}; Max-Age={max_age}; SameSite=Strict",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if http_only {
        value.push_str("; HttpOnly");
    }
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}

pub(crate) fn append_clear_cookie(headers: &mut HeaderMap, secure: bool) {
    let mut value = String::from(
        "auction_public_session=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; HttpOnly; SameSite=Lax",
    );
    if secure {
        value.push_str("; Secure");
    }
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(header::SET_COOKIE, value);
    }
}

pub(crate) fn append_clear_admin_cookies(headers: &mut HeaderMap, secure: bool) {
    for (name, http_only) in [(ADMIN_SESSION_COOKIE, true), (ADMIN_CSRF_COOKIE, false)] {
        let mut value = format!(
            "{name}=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; SameSite=Strict"
        );
        if http_only {
            value.push_str("; HttpOnly");
        }
        if secure {
            value.push_str("; Secure");
        }
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.append(header::SET_COOKIE, value);
        }
    }
}

pub(crate) fn public_session_cookie(
    token: &str,
    expires_at: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    secure: bool,
) -> Result<HeaderValue, ()> {
    let max_age = (expires_at - now).num_seconds().max(0);
    let mut value = format!(
        "{PUBLIC_SESSION_COOKIE}={token}; Path=/; Expires={}; Max-Age={max_age}; HttpOnly; SameSite=Lax",
        expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|_| ())
}
