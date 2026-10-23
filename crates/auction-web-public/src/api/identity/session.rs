//! Public 会话 HTTP DTO 与同源请求。
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
//! 组件只持有安全身份投影。

pub(crate) use auction_contracts::PublicSessionCurrentResponse as PublicSession;
use auction_contracts::{PublicLoginRequest, PublicLoginResponse};
use dioxus::{core::Runtime, prelude::*};
use gloo_net::http::{Request, Response};
use serde_json::Value;
use uuid::{Uuid, Variant};

use super::{TransportError, decoder, send_get, send_request};

/// 外壳会话重读入口；参数为 true 时先失效旧身份，阻止继续使用旧确认宿主。
#[derive(Clone, Copy)]
pub(crate) struct PublicSessionCheckTrigger(pub(crate) EventHandler<bool>);

/// 标签页间只广播随机修订，不保存用户身份或凭据。
#[cfg(target_arch = "wasm32")]
pub(crate) const PUBLIC_SESSION_CHANGE_STORAGE_KEY: &str = "hv.public.session-revision";

/// 认证失败或身份前置条件冲突时，重新读取当前 Cookie 对应的权威身份。
pub(super) fn observe_public_session_failure(status: u16, code: &str) {
    if status == 401 || code == "session_changed" {
        notify_public_session_change(true);
    }
}

pub(super) fn notify_public_session_change(invalidate: bool) {
    let trigger = Runtime::try_current()
        .filter(|runtime| runtime.try_current_scope_id().is_some())
        .and_then(|_| try_consume_context::<PublicSessionCheckTrigger>());
    if let Some(trigger) = trigger {
        trigger.0.call(invalidate);
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.set_item(PUBLIC_SESSION_CHANGE_STORAGE_KEY, &Uuid::new_v4().to_string());
    }
}

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
    let response = send_get("/api/v1/session").await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        let code = decoder::decode_error_code(response).await;
        return if code == "unauthenticated" { Ok(None) } else { Err(code) };
    }
    let payload: Option<PublicSession> =
        decoder::decode_success(response, valid_current_session_value).await?;
    match payload {
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
            // 请求体建立失败不携带 HTTP 状态，按网络失败分类。
            .map_err(|_| TransportError::Network(None))
    })
    .await
    // 结构化传输错误直接映射为稳定 transport code，不再比较中文字符串。
    .map_err(|error| PublicSessionApiError::local(error.code()))?;
    if !response.ok() {
        return Err(parse_api_error(response).await);
    }
    // Cookie 已由响应写入；即使正文解码失败也重读。重登同账号不能清除原未决命令。
    notify_public_session_change(false);
    let payload: PublicLoginResponse = decoder::decode_success(response, valid_login_session_value)
        .await
        .map_err(|_| PublicSessionApiError::local("invalid_response"))?;
    if !valid_login_session(&payload) {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    Ok(PublicSession {
        authenticated: true,
        user_id: payload.user_id,
        username: payload.username,
        expires_at: payload.expires_at,
        absolute_expires_at: payload.absolute_expires_at,
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

pub(crate) async fn logout_public(user_id: String) -> Result<(), String> {
    let response = send_request(|abort_signal| {
        Request::delete("/api/v1/session")
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &user_id)
            .build()
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        let status = response.status();
        let code = decoder::decode_error_code(response).await;
        observe_public_session_failure(status, &code);
        return Err(code);
    }
    notify_public_session_change(true);
    decoder::decode_success::<Value, _>(response, Value::is_null).await.map(|_| ())
}

fn valid_current_session(session: &PublicSession) -> bool {
    session.authenticated
        && valid_session_fields(
            &session.user_id,
            &session.username,
            session.expires_at.as_deref(),
            session.absolute_expires_at.as_deref(),
        )
}

fn valid_login_session(session: &PublicLoginResponse) -> bool {
    session.status == "active"
        && valid_session_fields(
            &session.user_id,
            &session.username,
            session.expires_at.as_deref(),
            session.absolute_expires_at.as_deref(),
        )
}

fn valid_current_session_value(value: &Value) -> bool {
    value.is_null()
        || has_exact_fields(
            value,
            &["authenticated", "user_id", "username", "expires_at", "absolute_expires_at"],
        )
}

fn valid_login_session_value(value: &Value) -> bool {
    has_exact_fields(value, &["user_id", "username", "status", "expires_at", "absolute_expires_at"])
}

fn has_exact_fields(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}

fn valid_session_fields(
    user_id: &str,
    username: &str,
    expires_at: Option<&str>,
    absolute_expires_at: Option<&str>,
) -> bool {
    valid_legacy_uuid(user_id)
        && !username.is_empty()
        && username == username.trim()
        && username.len() <= 256
        && !username.chars().any(char::is_control)
        && valid_session_deadlines(expires_at, absolute_expires_at)
}

fn valid_legacy_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.to_string() == value
            && (1..=5).contains(&id.get_version_num())
            && id.get_variant() == Variant::RFC4122
    })
}

/// 永久会话必须同时为空；有限会话只接受完整 RFC3339 且截止不得晚于绝对截止。
fn valid_session_deadlines(expires_at: Option<&str>, absolute_expires_at: Option<&str>) -> bool {
    match (expires_at, absolute_expires_at) {
        (None, None) => true,
        (Some(expires_at), Some(absolute_expires_at)) => {
            match (
                chrono::DateTime::parse_from_rfc3339(expires_at),
                chrono::DateTime::parse_from_rfc3339(absolute_expires_at),
            ) {
                (Ok(expires_at), Ok(absolute_expires_at)) => expires_at <= absolute_expires_at,
                _ => false,
            }
        }
        _ => false,
    }
}
