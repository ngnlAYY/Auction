use super::*;

pub(crate) const PUBLIC_SESSION_COOKIE: &str = "auction_public_session";

pub(crate) async fn login_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, true) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let input: PublicLoginRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let now = Utc::now();
    match session.login(input.key.into_bytes(), now).await {
        Ok(result) => {
            let mut response_headers = no_store_headers();
            let Ok(cookie) = public_session_cookie(
                &result.token,
                result.identity.absolute_expires_at,
                now,
                state.security().cookie_secure(),
            ) else {
                return internal_error();
            };
            response_headers.insert(header::SET_COOKIE, cookie);
            let data = PublicLoginResponse {
                user_id: result.identity.user_id.to_string(),
                username: result.identity.username,
                status: "active".to_owned(),
                expires_at: public_timestamp(result.identity.expires_at),
                absolute_expires_at: public_timestamp(result.identity.absolute_expires_at),
            };
            (StatusCode::OK, response_headers, Json(ApiResponse::ok("登录成功", data)))
                .into_response()
        }
        Err(error) => map_login_error(error),
    }
}

pub(crate) async fn current_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, false, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return current_session_response(&state, None, true);
    };
    match session.current(raw_token, Utc::now()).await {
        Ok(identity) => current_session_response(&state, Some(identity), false),
        Err(error) if is_unauthenticated(&error) => current_session_response(&state, None, true),
        Err(_) => session_unavailable(),
    }
}

pub(crate) async fn logout_session(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return authentication_error();
    };
    let now = Utc::now();
    match session.current(raw_token.clone(), now).await {
        Ok(identity) => {
            if let Err(response) = check_public_expected_user(&headers, identity.user_id) {
                return *response;
            }
        }
        Err(error) if is_unauthenticated(&error) => return authentication_error(),
        Err(_) => return session_unavailable(),
    }
    if session.logout(raw_token, now).await.is_err() {
        return session_unavailable();
    }
    let mut response_headers = no_store_headers();
    append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    (StatusCode::OK, response_headers, Json(ApiResponse::ok("会话已注销", Option::<()>::None)))
        .into_response()
}
