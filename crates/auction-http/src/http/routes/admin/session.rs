//! Admin 会话登录、当前会话读取和注销处理器。

use super::super::*;
use super::{ADMIN_CSRF_COOKIE, ADMIN_SESSION_COOKIE};

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
        state.security().login_failed();
        return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
            .into_response();
    };
    let now = Utc::now();
    match session.login(actor, secret, now).await {
        Ok(result) => {
            state.security().login_succeeded();
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
        Err(error) => {
            let response = map_admin_session_error(error);
            if response.status() == StatusCode::UNAUTHORIZED {
                state.security().login_failed();
            }
            response
        }
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
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_admin_cookie(&headers, ADMIN_SESSION_COOKIE) else {
        return authentication_error();
    };
    let now = Utc::now();
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
