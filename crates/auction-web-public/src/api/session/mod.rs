//! Public 会话 HTTP DTO 与同源请求。
//! 原始登录 key 只存在于浏览器表单和一次 HTTPS 请求中，组件只持有安全身份投影。

mod cookie;
mod identity;
mod transport;

use auction_contracts::PublicLoginResponse;
pub(crate) use auction_contracts::PublicSessionCurrentResponse as PublicSession;
use auction_web_shared::wire::json_has_exact_keys as has_exact_fields;
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
pub(crate) use self::identity::PUBLIC_SESSION_CHANGE_STORAGE_KEY;
use self::identity::observe_expected_user_rejection;
pub(crate) use self::identity::{
    PublicSessionCheckTrigger, PublicSessionEvidenceTrigger, SessionIdentity,
    SessionIdentityEvidence, SessionRefreshReason,
};
pub(super) use self::identity::{
    SessionIdentityObservation, notify_public_session_change, observe_public_session_failure,
    publish_cookie_identity,
};
#[cfg(not(test))]
use self::transport::SessionRequest;
use self::transport::send_session_request;
#[cfg(test)]
pub(crate) use self::transport::{SessionHttpFixture, SessionRequest};
pub(super) use self::transport::{decode_account_identity_response, decode_identity_response};

/// 所有可能写入 Public Cookie 的请求必须先持有同一原生跨标签页锁。
pub(super) async fn lock_cookie_mutation() -> Result<auction_web_shared::BrowserLock, &'static str>
{
    cookie::lock().await
}

/// 偏好写入可能只提交数据库、未收到响应；下次权威读取须重新核对浏览器持久期。
pub(super) fn invalidate_cookie_sync() {
    cookie::invalidate();
}

/// Public 会话失败的稳定投影；界面只根据错误码和安全公开原因选择文案。
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

    pub(crate) fn into_read_code(self) -> String {
        self.code.unwrap_or_else(|| "invalid_response".to_owned())
    }
}

/// 每次解码成功的 GET 都先报告权威身份。需要写 Cookie 时，必须在锁内再读一遍。
pub(crate) async fn fetch_public_session() -> Result<Option<PublicSession>, PublicSessionApiError> {
    let Some(current) = read_session_identity().await? else {
        cookie::invalidate();
        return Ok(None);
    };
    if !cookie::needs_sync(&current) {
        return Ok(Some(current));
    }
    let _cookie_lock = lock_session_request().await?;
    // 等锁期间另一标签页可能已经登录，必须用锁内读取绑定 PUT 的预期用户。
    let Some(current) = read_session_identity().await? else {
        cookie::invalidate();
        return Ok(None);
    };
    if !cookie::needs_sync(&current) {
        return Ok(Some(current));
    }
    let synchronized = request_current_session(SessionRequest::Sync(&current.user_id))
        .await
        .inspect_err(|error| {
            if observe_expected_user_rejection(
                error.status,
                error.code.as_deref(),
                Some(&current.user_id),
            ) {
                cookie::invalidate();
            }
        })?;
    let Some(synchronized) = synchronized.filter(|identity| identity.user_id == current.user_id)
    else {
        return Err(PublicSessionApiError::local("invalid_response"));
    };
    cookie::record(&synchronized);
    publish_cookie_identity(SessionIdentity::Snapshot(Some(synchronized.clone())));
    Ok(Some(synchronized))
}

async fn read_session_identity() -> Result<Option<PublicSession>, PublicSessionApiError> {
    let observation = SessionIdentityObservation::start();
    let current = request_current_session(SessionRequest::Read).await?;
    if !observation.publish(SessionIdentity::Snapshot(current.clone())) {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    Ok(current)
}

async fn request_current_session(
    request: SessionRequest<'_>,
) -> Result<Option<PublicSession>, PublicSessionApiError> {
    let response = send_session_request(request).await?;
    let status = response.status();
    let body = response.text().await;
    let current = decode_identity_response::<Option<PublicSession>>(
        status,
        body.as_deref(),
        valid_current_session_value,
    )?;
    if current.as_ref().is_some_and(|identity| !valid_current_session(identity)) {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    Ok(current)
}

async fn lock_session_request()
-> Result<Option<auction_web_shared::BrowserLock>, PublicSessionApiError> {
    #[cfg(test)]
    if dioxus::prelude::try_consume_context::<SessionHttpFixture>().is_some() {
        return Ok(None);
    }
    lock_cookie_mutation().await.map(Some).map_err(PublicSessionApiError::local)
}

pub(crate) async fn login_public(key: String) -> Result<PublicSession, PublicSessionApiError> {
    let _cookie_lock = lock_session_request().await?;
    cookie::invalidate();
    let response =
        send_session_request(SessionRequest::Login(&key)).await.inspect_err(|_error| {
            notify_public_session_change(SessionRefreshReason::Refresh);
        })?;
    let status = response.status();
    let body = response.text().await;
    let result = decode_identity_response::<PublicLoginResponse>(
        status,
        body.as_deref(),
        valid_login_session_value,
    )
    .and_then(|payload| {
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
    });
    if let Ok(identity) = &result {
        cookie::record(identity);
        publish_cookie_identity(SessionIdentity::Snapshot(Some(identity.clone())));
    }
    if (200..300).contains(&status) {
        // 成功状态已可能换了 Cookie。只有解码成功才能按新身份建立宿主，其余先复核。
        super::swr_cache::invalidate_all();
        notify_public_session_change(SessionRefreshReason::Refresh);
    }
    result
}

pub(crate) async fn logout_public(user_id: String) -> Result<(), String> {
    let _cookie_lock =
        lock_session_request().await.map_err(PublicSessionApiError::into_read_code)?;
    cookie::invalidate();
    let response = send_session_request(SessionRequest::Logout(&user_id))
        .await
        .inspect_err(|_error| {
            notify_public_session_change(SessionRefreshReason::Refresh);
        })
        .map_err(PublicSessionApiError::into_read_code)?;
    let status = response.status();
    let body = response.text().await;
    let result = decode_identity_response::<()>(status, body.as_deref(), Value::is_null);
    if result.is_ok() {
        publish_cookie_identity(SessionIdentity::Snapshot(None));
        super::swr_cache::invalidate_all();
    }
    if (200..300).contains(&status) {
        notify_public_session_change(SessionRefreshReason::Refresh);
    } else if let Err(error) = &result {
        observe_public_session_failure(
            status,
            error.code.as_deref().unwrap_or("invalid_response"),
            Some(&user_id),
        );
    }
    result.map_err(PublicSessionApiError::into_read_code)
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
    auction_web_shared::valid_uuid(user_id)
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
                auction_time::Timestamp::parse_from_rfc3339(expires_at),
                auction_time::Timestamp::parse_from_rfc3339(absolute_expires_at),
            ) {
                (Ok(expires_at), Ok(absolute_expires_at)) => expires_at <= absolute_expires_at,
                _ => false,
            }
        }
        _ => false,
    }
}
