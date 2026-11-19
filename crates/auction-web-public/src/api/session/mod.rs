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
pub(crate) use self::identity::{
    PublicSessionCheckTrigger, PublicSessionEvidenceTrigger, SessionIdentity,
    SessionIdentityEvidence, SessionRefreshReason,
};
pub(super) use self::identity::{
    SessionIdentityObservation, broadcast_session_change, notify_public_session_change,
    observe_public_session_failure, publish_cookie_identity, request_public_session_check,
};
use self::identity::{
    mark_public_login, mark_public_logout, observe_expected_user_rejection, publish_read_identity,
};
#[cfg(not(test))]
pub(super) use self::transport::SessionRequest;
pub(super) use self::transport::send_session_request;
#[cfg(test)]
pub(crate) use self::transport::{SessionHttpFixture, SessionRequest};
pub(super) use self::transport::{
    decode_account_identity_response, decode_identity_response, decode_observed_account_read,
};

/// 所有可能写入 Public Cookie 的请求必须先持有同一原生跨标签页锁。
pub(super) use self::cookie::lock as lock_cookie_mutation;

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

    /// 错误信封有效仍须匹配写入路由允许的冲突码，才能否定未知结果。
    pub(super) fn validate_conflict(mut self, known: &[&str]) -> Self {
        if self.status == Some(409)
            && !self.code.as_deref().is_some_and(|code| known.contains(&code))
        {
            self.code = Some("invalid_response".to_owned());
        }
        self
    }
}

/// 会话读取排在 Cookie 写锁之后，读取匿名时能观察到主动退出的结论。
#[derive(Default)]
pub(crate) struct SessionReadRevisions {
    pub primary: Option<String>,
    pub unknown_logout: Option<String>,
    pub login: Option<String>,
}

/// 权威会话绑定取得锁后、发出 GET 前观察的共享主修订。
pub(crate) async fn fetch_public_session(
    reason: SessionRefreshReason,
    observe_confirmed_logout: impl Fn(Option<String>),
) -> (Result<Option<PublicSession>, PublicSessionApiError>, SessionReadRevisions) {
    let mut revisions = SessionReadRevisions::default();
    let result = fetch_session(reason, || {
        let (confirmed, unknown) = PublicSessionCheckTrigger::logout_revisions();
        observe_confirmed_logout(confirmed);
        revisions.unknown_logout = unknown;
        revisions.login = PublicSessionCheckTrigger::login_revision();
        revisions.primary = PublicSessionCheckTrigger::session_revision();
        revisions.primary.clone()
    })
    .await;
    (result, revisions)
}

async fn fetch_session(
    reason: SessionRefreshReason,
    observe_revisions: impl FnOnce() -> Option<String>,
) -> Result<Option<PublicSession>, PublicSessionApiError> {
    let lock = lock_cookie_mutation().await;
    let revision = observe_revisions();
    let _cookie_lock = match lock {
        Ok(lock) => lock,
        Err("session_lock_unavailable") => {
            // 旧浏览器仍能读取；没有原生锁时不能以本地顺序冒充跨页独占写入。
            let current = read_session_identity(false, revision.as_deref()).await?;
            if reason.synchronizes_cookie() && current.as_ref().is_some_and(cookie::needs_sync) {
                return Err(PublicSessionApiError::local("session_lock_unavailable"));
            }
            return Ok(current);
        }
        Err(code) => return Err(PublicSessionApiError::local(code)),
    };
    // 等锁期间 Cookie 可能已经换号。首次锁内读回的身份是 PUT 前置条件，锁持有到响应收口。
    let current = read_session_identity(true, revision.as_deref()).await?;
    match current {
        Some(current) if reason.synchronizes_cookie() && cookie::needs_sync(&current) => {
            synchronize_cookie(current).await.map(Some)
        }
        current => Ok(current),
    }
}

async fn synchronize_cookie(
    current: PublicSession,
) -> Result<PublicSession, PublicSessionApiError> {
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
    Ok(synchronized)
}

async fn read_session_identity(
    locked: bool,
    revision: Option<&str>,
) -> Result<Option<PublicSession>, PublicSessionApiError> {
    let observation = (!locked).then(SessionIdentityObservation::start);
    let current = request_current_session(SessionRequest::Read).await?;
    if revision != PublicSessionCheckTrigger::session_revision().as_deref() {
        return Err(PublicSessionApiError::local("invalid_response"));
    }
    let evidence = SessionIdentity::Snapshot(current.clone());
    if let Some(observation) = observation {
        if !observation.publish(evidence) {
            return Err(PublicSessionApiError::local("invalid_response"));
        }
    } else {
        publish_read_identity(evidence);
    }
    if current.is_none() {
        cookie::invalidate();
    }
    Ok(current)
}

async fn request_current_session(
    request: SessionRequest<'_>,
) -> Result<Option<PublicSession>, PublicSessionApiError> {
    let response = send_session_request(request)
        .await
        .map_err(|failure| PublicSessionApiError::local(failure.transport.code()))?;
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

pub(crate) async fn login_public(key: String) -> Result<PublicSession, PublicSessionApiError> {
    let cookie_lock = lock_cookie_mutation().await.map_err(PublicSessionApiError::local)?;
    cookie::invalidate();
    let response = send_session_request(SessionRequest::Login(&key)).await;
    drop(key);
    let response = match response {
        Ok(response) => response,
        Err(failure) => {
            if failure.needs_reconciliation() {
                broadcast_session_change();
            }
            drop(cookie_lock);
            if failure.needs_reconciliation() {
                request_public_session_check(SessionRefreshReason::CookieChanged);
            }
            return Err(PublicSessionApiError::local(failure.transport.code()));
        }
    };
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
    let recheck = (200..300).contains(&status)
        || status >= 500
        || (status == 409
            && result
                .as_ref()
                .is_err_and(|error| error.code.as_deref() == Some("invalid_response")));
    if recheck {
        // 下一个写者取得锁后必须能看见本次 Cookie 写入的修订。
        broadcast_session_change();
    }
    if let Ok(identity) = &result {
        cookie::record(identity);
        mark_public_login();
        publish_cookie_identity(SessionIdentity::Snapshot(Some(identity.clone())));
    }
    drop(cookie_lock);
    if recheck {
        // 只对账当前 Cookie。失败不能证明换号，也不能再次提交登录 key 或失效公开读缓存。
        request_public_session_check(SessionRefreshReason::CookieChanged);
    }
    result
}

pub(crate) async fn logout_public(user_id: String) -> Result<(), String> {
    let cookie_lock = lock_cookie_mutation().await.map_err(str::to_owned)?;
    cookie::invalidate();
    let response = match send_session_request(SessionRequest::Logout(&user_id)).await {
        Ok(response) => response,
        Err(failure) => {
            if failure.needs_reconciliation() {
                mark_public_logout(false);
                broadcast_session_change();
                drop(cookie_lock);
                request_public_session_check(SessionRefreshReason::LogoutUnknown);
            }
            return Err(failure.transport.code().to_owned());
        }
    };
    let status = response.status();
    let body = response.text().await;
    let result = decode_identity_response::<()>(status, body.as_deref(), Value::is_null)
        .map_err(|error| error.validate_conflict(&["session_changed"]));
    let reason = if result.is_ok() {
        mark_public_logout(true);
        publish_cookie_identity(SessionIdentity::Snapshot(None));
        Some(SessionRefreshReason::CookieChanged)
    } else if (200..300).contains(&status)
        || status >= 500
        || (status == 409
            && result
                .as_ref()
                .is_err_and(|error| error.code.as_deref() == Some("invalid_response")))
    {
        // 只有完整回执才能确认退出；未知标记仅抑制一次提示，仍保留最后确认的命令宿主。
        mark_public_logout(false);
        Some(SessionRefreshReason::LogoutUnknown)
    } else {
        None
    };
    if let Some(reason) = reason {
        broadcast_session_change();
        drop(cookie_lock);
        request_public_session_check(reason);
    } else {
        drop(cookie_lock);
        if let Err(error) = &result {
            observe_public_session_failure(
                status,
                error.code.as_deref().unwrap_or("invalid_response"),
                Some(&user_id),
            );
        }
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::ready;

    use auction_contracts::{ApiResponse, PublicLoginResponse};
    use dioxus::prelude::*;

    use super::{SessionHttpFixture, SessionRequest, login_public, logout_public};
    use crate::api::clock::HttpResponseFixture;
    use crate::api::swr_cache::{FetchOutcome, get_or_fetch};

    const USER: &str = "018f4c8e-7b2a-4c3d-8e4f-000000000001";
    const URL: &str = "/api/v1/auctions?world=main&status=settled&page=713";

    thread_local! {
        static PASSED: Cell<bool> = const { Cell::new(false) };
    }

    #[component]
    fn CachedPublicRead() -> Element {
        use_context_provider(|| SessionHttpFixture {
            respond: |request| {
                let body = match request {
                    SessionRequest::Login(_) => serde_json::to_string(&ApiResponse::ok(
                        "登录成功",
                        PublicLoginResponse {
                            user_id: USER.to_owned(),
                            username: "用户".to_owned(),
                            status: "active".to_owned(),
                            expires_at: None,
                            absolute_expires_at: None,
                        },
                    ))
                    .unwrap(),
                    SessionRequest::Logout(_) => {
                        r#"{"code":"ok","message":"已退出","data":null}"#.to_owned()
                    }
                    _ => panic!("公开快照验证只运行登录和退出命令"),
                };
                Box::pin(ready(Ok(HttpResponseFixture {
                    status: 200,
                    etag: None,
                    body: Some(body),
                })))
            },
            wait_for_lock: || Box::pin(ready(())),
            track_lock: |_| {},
            get_revision: |_| None,
            set_revision: |_| {},
        });
        use_future(|| async {
            let cached = get_or_fetch(
                URL,
                15,
                || 1,
                |_| ready(FetchOutcome::<_, ()>::Changed { value: "已结算场次", etag: None }),
            )
            .await;
            assert_eq!(cached, Ok("已结算场次"));
            login_public("fixture".to_owned()).await.unwrap();
            let after_login =
                get_or_fetch(URL, 15, || 2, |_| async { panic!("登录使新鲜的公开读缓存过期") })
                    .await;
            assert_eq!(after_login, cached);
            logout_public(USER.to_owned()).await.unwrap();
            let after_logout =
                get_or_fetch(URL, 15, || 3, |_| async { panic!("退出使新鲜的公开读缓存过期") })
                    .await;
            assert_eq!(after_logout, cached);
            PASSED.set(true);
        });
        rsx! {}
    }

    #[test]
    fn login_and_logout_preserve_a_fresh_public_read() {
        PASSED.set(false);
        let mut dom = VirtualDom::new(CachedPublicRead);
        dom.rebuild_in_place();
        dom.process_events();
        assert!(PASSED.get());
    }

    #[test]
    fn current_session_decodes_backend_contract_fixtures() {
        for (body, authenticated, timed) in [
            (
                include_str!("../../../../auction-contracts/tests/fixtures/public/session.json"),
                true,
                true,
            ),
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/session-permanent.json"
                ),
                true,
                false,
            ),
            (
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/session-anonymous.json"
                ),
                false,
                false,
            ),
        ] {
            let session = super::decode_identity_response::<Option<super::PublicSession>>(
                200,
                Some(body),
                super::valid_current_session_value,
            )
            .expect("当前会话");
            assert_eq!(session.is_some(), authenticated);
            if let Some(session) = session {
                assert!(super::valid_current_session(&session));
                assert_eq!(session.expires_at.is_some(), timed);
            }
        }
    }

    #[test]
    fn login_errors_decode_backend_contract_fixtures() {
        for (status, code, body) in [
            (
                400,
                "invalid_key",
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/error-invalid-key.json"
                ),
            ),
            (
                401,
                "invalid_credentials",
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/error-invalid-credentials.json"
                ),
            ),
            (
                403,
                "account_blacklisted",
                include_str!(
                    "../../../../auction-contracts/tests/fixtures/public/error-account-blacklisted.json"
                ),
            ),
        ] {
            let error = super::decode_identity_response::<auction_contracts::PublicLoginResponse>(
                status,
                Some(body),
                super::valid_login_session_value,
            )
            .expect_err("登录已被拒绝");
            assert_eq!(error.code.as_deref(), Some(code));
            assert_eq!(error.public_reason.as_deref(), (status == 403).then_some("示例封禁原因"));
        }
    }
}
