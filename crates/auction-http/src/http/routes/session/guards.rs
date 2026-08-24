//! Public/Admin 会话限流、来源检查和身份认证守卫。

use super::*;
use super::{
    cookies::{read_admin_cookie, read_public_session_cookie},
    errors::{authentication_error, is_unauthenticated, session_unavailable},
};

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
