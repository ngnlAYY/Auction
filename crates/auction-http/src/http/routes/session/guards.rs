//! Public/Admin 会话限流、来源检查和身份认证守卫。

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
    authenticated_admin_session(state, headers).await
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
        Ok(identity) => Ok(identity),
        Err(error) if is_unauthenticated(&error) => Err(Box::new(authentication_error())),
        Err(_) => Err(Box::new(competition_unavailable())),
    }
}
