//! 登录时长偏好的账号边界；会话与版本复核、幂等和截止计算归应用及存储 owner。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use crate::mapping::{api_error, parse_public_uuid, public_timestamp};
use crate::middleware::strict_json;
use crate::session::{
    authenticated_public_mutation_identity, authentication_error, internal_error,
    is_unauthenticated, no_store_headers, public_session_cookie, read_public_session_cookie,
    session_guard, session_unavailable,
};
use crate::state::PublicApiState;
use auction_application::core::{ApplicationError, SessionError};
use auction_application::public::{
    PublicSessionPreferenceResult, SetPublicSessionPreferenceCommand,
};
use auction_contracts::ApiResponse;
use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use auction_domain::CommandId;
use axum::Json;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use std::net::SocketAddr;

pub(crate) async fn get_session_preference(
    State(state): State<PublicApiState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&headers) else {
        return authentication_error();
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    match session.get_preference(raw_token, Utc::now()).await {
        Ok(value) if valid(&value) => success(value),
        Ok(_) => session_unavailable(),
        Err(error) => failure(error),
    }
}

pub(crate) async fn set_session_preference(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Some(response) = session_guard(&state, &parts.headers, address, true, false) {
        return response;
    }
    let identity = match authenticated_public_mutation_identity(&state, &parts.headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    if parts.uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let input: PublicSetSessionPreferenceRequest =
        match strict_json::decode_after_guard(&parts.headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_id) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    let Some(session) = state.session() else {
        return session_unavailable();
    };
    let Some(raw_token) = read_public_session_cookie(&parts.headers) else {
        return authentication_error();
    };
    let result = session
        .set_preference(
            raw_token.clone(),
            SetPublicSessionPreferenceCommand {
                command_id: CommandId::from_uuid(command_id),
                user_id: identity.user_id,
                duration_seconds: input.duration_seconds,
                expected_version: input.expected_version,
            },
            Utc::now(),
        )
        .await;
    let value = match result {
        Ok(value)
            if valid(&value)
                && value.preference.user_id == identity.user_id
                && value.preference.duration_seconds == input.duration_seconds
                && Some(value.preference.version) == input.expected_version.checked_add(1) =>
        {
            value
        }
        Ok(_) => return session_unavailable(),
        Err(error) => return failure(error),
    };
    // 重放返回原命令结果，但后续命令可能已改变当前期限；Cookie 必须使用新权威快照，不能回写历史截止。
    // 该响应仍立即同步 Cookie，避免有限延长/改永久被旧浏览器截止截断；浏览器须持有登录共用锁。
    let now = Utc::now();
    let current = match session.inspect(raw_token.clone(), now).await {
        Ok(current) if current.user_id == identity.user_id => current,
        Ok(_) => return session_unavailable(),
        Err(error) => return failure(error),
    };
    let Ok(cookie) = public_session_cookie(
        &raw_token,
        current.effective_expires_at(),
        now,
        state.security().cookie_secure(),
    ) else {
        return internal_error();
    };
    let mut response = success(value);
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}

fn valid(value: &PublicSessionPreferenceResult) -> bool {
    value.preference.user_id == value.identity.user_id
        && value.preference.validate().is_ok()
        && value.identity.expires_at.is_some() == value.identity.absolute_expires_at.is_some()
        && value.identity.expires_at <= value.identity.absolute_expires_at
}

fn success(value: PublicSessionPreferenceResult) -> Response {
    let data = PublicSessionPreferenceResponse {
        user_id: value.preference.user_id.to_string(),
        duration_seconds: value.preference.duration_seconds,
        version: value.preference.version,
        expires_at: value.identity.effective_expires_at().map(public_timestamp),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("登录时长已确认", data)))
        .into_response()
}

fn failure(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::Session(SessionError::InvalidPreference) => api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_session_preference",
            "登录时长或版本无效",
        )
        .into_response(),
        ApplicationError::Session(SessionError::PreferenceConflict) => {
            api_error(StatusCode::CONFLICT, "session_preference_conflict", "登录时长命令或版本冲突")
                .into_response()
        }
        error if is_unauthenticated(&error) => authentication_error(),
        _ => session_unavailable(),
    };
    let response = body();
    diagnostics::mapped_response(
        "session_preference",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}
