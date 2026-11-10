//! Public 会话 HTTP DTO 与同源请求。
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中；成功后由服务端设置 HttpOnly Cookie，
//! 组件只持有安全身份投影。

mod cookie;
pub(crate) use auction_contracts::PublicSessionCurrentResponse as PublicSession;
use auction_contracts::{PublicLoginRequest, PublicLoginResponse};
use dioxus::{core::Runtime, prelude::*};
use gloo_net::http::{Request, Response};
use serde_json::Value;

use auction_web_shared::wire::json_has_exact_keys as has_exact_fields;

use super::{TransportError, decoder, send_get, send_request};

/// 所有可能写入 Public Cookie 的请求必须先持有同一原生跨标签页锁。
pub(super) async fn lock_cookie_mutation() -> Result<auction_web_shared::BrowserLock, &'static str>
{
    cookie::lock().await
}

/// 偏好写入可能只提交数据库、未收到响应；下次权威读取须重新核对浏览器持久期。
pub(super) fn invalidate_cookie_sync() {
    cookie::invalidate();
}

/// 会话重读原因。认证失败要先失效旧身份并提示服务端撤销；本浏览器主动退出只同步为匿名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionRefreshReason {
    /// 主动重读（聚焦、可见性、标签页广播、截止到期、登录或偏好保存）：保留原快照。
    Refresh,
    /// 认证失败（401 或 `session_changed`）：先失效旧身份，并视为服务端撤销。
    AuthenticationFailed,
    /// 本浏览器主动退出：先失效旧身份，但不提示“已在其他设备登录”。
    LoggedOut,
}

impl SessionRefreshReason {
    /// 已知认证变化必须先卸载旧用户命令，阻止继续使用旧确认宿主。
    pub(crate) fn invalidates(self) -> bool {
        !matches!(self, Self::Refresh)
    }

    /// 未到期却失效的认证会话只可能来自服务端撤销；主动退出与普通重读不提示。
    pub(crate) fn shows_ended_elsewhere(self) -> bool {
        matches!(self, Self::AuthenticationFailed)
    }
}

/// 外壳会话重读入口。
#[derive(Clone, Copy)]
pub(crate) struct PublicSessionCheckTrigger(pub(crate) EventHandler<SessionRefreshReason>);

/// 标签页间只广播随机修订，不保存用户身份或凭据。
#[cfg(target_arch = "wasm32")]
pub(crate) const PUBLIC_SESSION_CHANGE_STORAGE_KEY: &str = "hv.public.session-revision";

/// 认证失败或身份前置条件冲突时，重新读取当前 Cookie 对应的权威身份。
pub(super) fn observe_public_session_failure(status: u16, code: &str) {
    if status == 401 || code == "session_changed" {
        notify_public_session_change(SessionRefreshReason::AuthenticationFailed);
    }
}

pub(super) fn notify_public_session_change(reason: SessionRefreshReason) {
    let trigger = Runtime::try_current()
        .filter(|runtime| runtime.try_current_scope_id().is_some())
        .and_then(|_| try_consume_context::<PublicSessionCheckTrigger>());
    if let Some(trigger) = trigger {
        trigger.0.call(reason);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = auction_web_shared::storage::set_item(
        PUBLIC_SESSION_CHANGE_STORAGE_KEY,
        &uuid::Uuid::new_v4().to_string(),
    );
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

/// 普通读取不写 Cookie；只有首次读取、截止变化或持久期同步到期时进入显式串行化写路径。
pub(crate) async fn fetch_public_session() -> Result<Option<PublicSession>, String> {
    let current = fetch_public_session_once().await?;
    let Some(current) = current else {
        cookie::invalidate();
        return Ok(None);
    };
    if !cookie::needs_sync(&current) {
        return Ok(Some(current));
    }
    let _cookie_lock = lock_cookie_mutation().await.map_err(str::to_owned)?;
    // 等锁期间另一标签页可能已经登录 B；必须重读现 Cookie，不能把外层旧 A 投影交给 UI。
    let current = fetch_public_session_once().await?;
    let Some(current) = current else {
        cookie::invalidate();
        return Ok(None);
    };
    if !cookie::needs_sync(&current) {
        return Ok(Some(current));
    }
    let response = send_request(|abort_signal| {
        Request::put("/api/v1/session")
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, &current.user_id)
            .build()
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| error.code().to_owned())?;
    let successful = response.ok();
    let synchronized = decode_current_session(response).await?;
    match synchronized {
        Some(identity) if identity.user_id == current.user_id => {
            cookie::record(&identity);
            Ok(Some(identity))
        }
        None if !successful => {
            cookie::invalidate();
            Ok(None)
        }
        _ => Err("invalid_response".to_owned()),
    }
}

async fn fetch_public_session_once() -> Result<Option<PublicSession>, String> {
    let response = send_get("/api/v1/session").await.map_err(|error| error.code().to_owned())?;
    decode_current_session(response).await
}

async fn decode_current_session(response: Response) -> Result<Option<PublicSession>, String> {
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
    let _cookie_lock = lock_cookie_mutation().await.map_err(PublicSessionApiError::local)?;
    cookie::invalidate();
    let response = send_request(move |abort_signal| {
        Request::post("/api/v1/session")
            .abort_signal(abort_signal)
            .json(&PublicLoginRequest { key })
            // 请求体建立失败不携带 HTTP 状态，按网络失败分类。
            .map_err(|_| TransportError::Network(None))
    })
    .await
    // 结构化传输错误直接映射为稳定 transport code，不依赖中文文案比较。
    .map_err(|error| PublicSessionApiError::local(error.code()))?;
    if !response.ok() {
        return Err(parse_api_error(response).await);
    }
    // Cookie 已由响应写入；即使正文解码失败也重读。重登同账号不能清除原未决命令。
    notify_public_session_change(SessionRefreshReason::Refresh);
    let payload: PublicLoginResponse = decoder::decode_success(response, valid_login_session_value)
        .await
        .map_err(|_| PublicSessionApiError::local("invalid_response"))?;
    if !valid_login_session(&payload) {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    let session = PublicSession {
        authenticated: true,
        user_id: payload.user_id,
        username: payload.username,
        expires_at: payload.expires_at,
        absolute_expires_at: payload.absolute_expires_at,
    };
    cookie::record(&session);
    Ok(session)
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
    let _cookie_lock = lock_cookie_mutation().await.map_err(str::to_owned)?;
    cookie::invalidate();
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
    // 主动退出：其他标签页同步为匿名，但不提示被其他设备挤下线。
    notify_public_session_change(SessionRefreshReason::LoggedOut);
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

fn valid_session_fields(
    user_id: &str,
    username: &str,
    expires_at: Option<&str>,
    absolute_expires_at: Option<&str>,
) -> bool {
    auction_web_shared::is_legacy_uuid(user_id)
        && !username.is_empty()
        && username == username.trim()
        && username.len() <= 256
        && !username.chars().any(char::is_control)
        && valid_session_deadlines(expires_at, absolute_expires_at)
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
