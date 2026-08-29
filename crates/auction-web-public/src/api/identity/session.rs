//! Public 会话 HTTP DTO 与同源请求。
//!
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
//! 组件只持有安全身份投影。

pub(crate) use auction_contracts::PublicSessionCurrentResponse as PublicSession;
use auction_contracts::{ApiResponse, PublicLoginRequest, PublicLoginResponse};
use gloo_net::http::{Request, Response};
use serde_json::Value;
use uuid::{Uuid, Variant};

use super::{decoder, observe_response_date, send_get, send_request, transport_error_code};

/// Public 登录失败的稳定投影；界面只根据错误码和安全公开原因选择本地化文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublicSessionApiError {
    pub(crate) status: Option<u16>,
    pub(crate) code: Option<String>,
    pub(crate) public_reason: Option<String>,
}

impl PublicSessionApiError {
    /// 构造浏览器本地校验错误，不把用户输入或底层网络错误带入页面。
    pub(crate) fn local(code: &'static str) -> Self {
        Self { status: None, code: Some(code.to_owned()), public_reason: None }
    }
}

pub(crate) async fn fetch_public_session() -> Result<Option<PublicSession>, String> {
    fetch_public_session_once().await
}

async fn fetch_public_session_once() -> Result<Option<PublicSession>, String> {
    let response =
        send_get("/api/v1/session").await.map_err(|error| transport_error_code(&error))?;
    if !response.ok() {
        let code = decoder::decode_error_code(response).await;
        return if code == "unauthenticated" { Ok(None) } else { Err(code) };
    }
    let payload: ApiResponse<Option<PublicSession>> =
        response.json().await.map_err(|_| "invalid_response".to_owned())?;
    match payload.data {
        None => Ok(None),
        Some(session) if valid_current_session(&session) => Ok(Some(session)),
        Some(_) => Err("invalid_response".to_owned()),
    }
}

pub(crate) async fn login_public(key: String) -> Result<PublicSession, PublicSessionApiError> {
    let response = send_request(move |abort_signal| {
        Request::post("/api/v1/session")
            .abort_signal(abort_signal)
            .json(&PublicLoginRequest { key })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| {
        if error == "请求超时" {
            PublicSessionApiError::local("request_timeout")
        } else {
            PublicSessionApiError::local("network_error")
        }
    })?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(parse_api_error(response).await);
    }
    let payload: ApiResponse<PublicLoginResponse> =
        response.json().await.map_err(|_| PublicSessionApiError::local("invalid_response"))?;
    if !valid_login_session(&payload.data) {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    Ok(PublicSession {
        authenticated: true,
        user_id: payload.data.user_id,
        username: payload.data.username,
        expires_at: payload.data.expires_at,
        absolute_expires_at: payload.data.absolute_expires_at,
    })
}

async fn parse_api_error(response: Response) -> PublicSessionApiError {
    let status = Some(response.status());
    let Ok((code, data)) = decoder::decode_error(response).await else {
        return PublicSessionApiError {
            status,
            ..PublicSessionApiError::local("invalid_response")
        };
    };
    let public_reason = if status == Some(403) && code == "account_blacklisted" {
        data.as_object()
            .filter(|data| data.len() == 1)
            .and_then(|data| data.get("public_reason"))
            .and_then(Value::as_str)
            .filter(|reason| {
                !reason.is_empty()
                    && *reason == reason.trim()
                    && reason.len() <= 1024
                    && !reason.chars().any(char::is_control)
            })
            .map(ToOwned::to_owned)
    } else {
        None
    };
    PublicSessionApiError { status, code: Some(code), public_reason }
}

pub(crate) async fn logout_public() -> Result<(), String> {
    let response = send_request(|abort_signal| {
        Request::delete("/api/v1/session")
            .abort_signal(abort_signal)
            .build()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| transport_error_code(&error))?;
    observe_response_date(&response);
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_success::<Value, _>(response, Value::is_null).await.map(|_| ())
}

fn valid_current_session(session: &PublicSession) -> bool {
    session.authenticated
        && valid_session_fields(
            &session.user_id,
            &session.username,
            &session.expires_at,
            &session.absolute_expires_at,
        )
}

fn valid_login_session(session: &PublicLoginResponse) -> bool {
    session.status == "active"
        && valid_session_fields(
            &session.user_id,
            &session.username,
            &session.expires_at,
            &session.absolute_expires_at,
        )
}

fn valid_session_fields(
    user_id: &str,
    username: &str,
    expires_at: &str,
    absolute_expires_at: &str,
) -> bool {
    valid_legacy_uuid(user_id)
        && !username.is_empty()
        && username == username.trim()
        && username.len() <= 256
        && !username.chars().any(char::is_control)
        && valid_legacy_time(expires_at)
        && valid_legacy_time(absolute_expires_at)
}

fn valid_legacy_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.to_string() == value
            && (1..=5).contains(&id.get_version_num())
            && id.get_variant() == Variant::RFC4122
    })
}

#[cfg(target_arch = "wasm32")]
fn valid_legacy_time(value: &str) -> bool {
    !value.is_empty() && !js_sys::Date::parse(value).is_nan()
}

#[cfg(not(target_arch = "wasm32"))]
fn valid_legacy_time(value: &str) -> bool {
    !value.is_empty() && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}
