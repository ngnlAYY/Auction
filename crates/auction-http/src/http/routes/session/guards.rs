//! Public/Admin 会话限流、来源检查和身份认证守卫。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use auction_observability::HttpRateLimitKind;

use super::*;
use super::{
    cookies::{read_admin_cookie, read_public_session_cookie},
    errors::{
        authentication_error, competition_unavailable, is_unauthenticated, session_unavailable,
    },
};

pub(crate) fn session_guard(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else {
        state.security().allow_mutation(address.ip())
    };
    if let Some(retry_after) = retry_after {
        if let Some(metrics) = state.metrics() {
            metrics.http().observe_rate_limit_rejection(if login {
                HttpRateLimitKind::Login
            } else {
                HttpRateLimitKind::Mutation
            });
        }
        return Some(rate_limited_response(retry_after));
    }
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    None
}

/// Public WebSocket 的附加连接尝试门禁；通过后仍按旧版 origin→query→session 顺序处理。
pub(crate) fn realtime_connection_guard(
    state: &PublicApiState,
    headers: &HeaderMap,
    address: SocketAddr,
) -> Option<Response> {
    if let Some(retry_after) = state.security().allow_realtime_connection(address.ip()) {
        if let Some(metrics) = state.metrics() {
            metrics.http().observe_rate_limit_rejection(HttpRateLimitKind::Realtime);
        }
        return Some(rate_limited_response(retry_after));
    }
    if !state.security().allows_websocket_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    None
}

pub(crate) fn admin_session_guard(
    state: &AdminApiState,
    headers: &HeaderMap,
    address: SocketAddr,
    check_origin: bool,
    login: bool,
) -> Option<Response> {
    let retry_after = if login {
        state.security().allow_login(address.ip())
    } else if check_origin {
        state.security().allow_mutation(address.ip())
    } else {
        None
    };
    if let Some(retry_after) = retry_after {
        if let Some(metrics) = state.metrics() {
            metrics.http().observe_rate_limit_rejection(if login {
                HttpRateLimitKind::Login
            } else {
                HttpRateLimitKind::Mutation
            });
        }
        return Some(rate_limited_response(retry_after));
    }
    if check_origin && !state.security().allows_origin(headers) {
        return Some(
            api_error(StatusCode::FORBIDDEN, "origin_forbidden", "请求来源不受信任")
                .into_response(),
        );
    }
    None
}

fn rate_limited_response(retry_after: u64) -> Response {
    let mut response_headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&retry_after.max(1).to_string()) {
        response_headers.insert(header::RETRY_AFTER, value);
    }
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
        .into_response()
}

pub(crate) async fn authenticated_admin_session(
    state: &AdminApiState,
    headers: &HeaderMap,
) -> Result<(String, AdminSessionIdentity), Box<Response>> {
    authenticated_admin_session_with_csrf(state, headers, None).await
}

// 写守卫复用已经解析并核对请求头的 CSRF Cookie，保持 session 能力及 token 的错误顺序。
async fn authenticated_admin_session_with_csrf(
    state: &AdminApiState,
    headers: &HeaderMap,
    raw_csrf: Option<String>,
) -> Result<(String, AdminSessionIdentity), Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(session_unavailable()));
    };
    let Some(raw_token) = read_admin_cookie(headers, ADMIN_SESSION_COOKIE) else {
        return Err(Box::new(authentication_error()));
    };
    let Some(raw_csrf) = raw_csrf.or_else(|| read_admin_cookie(headers, ADMIN_CSRF_COOKIE)) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, raw_csrf.clone(), Utc::now()).await {
        Ok(identity) => Ok((raw_csrf, identity)),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "admin_session",
                "validate_csrf",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            Err(Box::new(session_unavailable()))
        }
    }
}

/// Admin 写入统一守卫；顺序与旧 v1 保持为限流 → Origin → CSRF → session。
pub(crate) async fn authenticated_admin_mutation(
    state: &AdminApiState,
    headers: &HeaderMap,
    address: SocketAddr,
) -> Result<(String, AdminSessionIdentity), Box<Response>> {
    if let Some(response) = admin_session_guard(state, headers, address, true, false) {
        return Err(Box::new(response));
    }
    let Some(raw_csrf) = read_admin_cookie(headers, ADMIN_CSRF_COOKIE) else {
        return Err(Box::new(
            api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response(),
        ));
    };
    if !valid_admin_csrf_header(headers, &raw_csrf) {
        return Err(Box::new(
            api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败").into_response(),
        ));
    }
    authenticated_admin_session_with_csrf(state, headers, Some(raw_csrf)).await
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
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_session",
                "authenticate_identity",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            Err(Box::new(session_unavailable()))
        }
    }
}

/// Public 账号写操作在当前 Cookie 认证后核对确认宿主。
pub(crate) async fn authenticated_public_mutation_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
) -> Result<PublicSessionIdentity, Box<Response>> {
    let identity = authenticated_public_identity(state, headers).await?;
    check_public_expected_user(headers, identity.user_id)?;
    Ok(identity)
}

/// Public 竞价命令认证守卫；认证失败仍是 401，依赖失败使用旧 v1 的通用 503 外壳。
pub(crate) async fn authenticated_public_competition_identity(
    state: &PublicApiState,
    headers: &HeaderMap,
) -> Result<PublicSessionIdentity, Box<Response>> {
    let Some(session) = state.session() else {
        return Err(Box::new(competition_unavailable()));
    };
    let Some(raw_token) = read_public_session_cookie(headers) else {
        return Err(Box::new(authentication_error()));
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => {
            check_public_expected_user(headers, identity.user_id)?;
            Ok(identity)
        }
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_competition",
                "authenticate_identity",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            Err(Box::new(competition_unavailable()))
        }
    }
}

/// 可选身份前置条件与当前 Cookie 核对，防止旧页面读取或操作切换后的账号。
/// 未提供请求头的旧客户端维持原 v1 语义；此条件不取代会话、Origin 或命令资格校验。
pub(crate) fn check_public_expected_user(
    headers: &HeaderMap,
    user_id: Uuid,
) -> Result<(), Box<Response>> {
    let mut values = headers.get_all(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(());
    };
    let expected = value.to_str().ok().and_then(|value| parse_public_uuid(value).ok());
    if values.next().is_some() || expected.is_none() {
        return Err(Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_expected_user_id", "预期会话身份无效")
                .into_response(),
        ));
    }
    if expected != Some(user_id) {
        return Err(Box::new(
            api_error(StatusCode::CONFLICT, "session_changed", "会话身份已变化，请重新确认操作")
                .into_response(),
        ));
    }
    Ok(())
}
