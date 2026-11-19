//! Public 会话请求及可用于身份证据的严格响应信封。

use auction_contracts::{
    ApiResponse, PUBLIC_EXPECTED_USER_ID_HEADER, PublicLoginRequest,
    PublicSetSessionPreferenceRequest,
};
use auction_web_shared::{BrowserErrorClass, report_browser_error};
use gloo_net::http::Request;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(test)]
use crate::api::clock::HttpFixtureFuture;
use crate::api::clock::{HttpResponse, send_response};
use crate::api::decoder::decode_error_body;
use crate::api::session_preference::PATH as SESSION_PREFERENCE_PATH;
use crate::api::{CommandError, TransportError};

use super::PublicSessionApiError;
use super::identity::observe_public_session_failure;

const PATH: &str = "/api/v1/session";

/// 会话读取与 Cookie 写入，携带的用户前置条件取自请求本身。
#[derive(Clone, Copy)]
pub(crate) enum SessionRequest<'a> {
    Read,
    Sync(&'a str),
    Login(&'a str),
    Logout(&'a str),
    ReadPreference,
    SetPreference(&'a str, &'a PublicSetSessionPreferenceRequest),
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct SessionHttpFixture {
    pub respond: fn(SessionRequest<'_>) -> HttpFixtureFuture,
    pub wait_for_lock: fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    pub track_lock: fn(bool),
    pub get_revision: fn(&str) -> Option<String>,
    pub set_revision: fn(&str),
}

/// 写入失败保留是否已构造请求及正文断流前收到的状态。
pub(in crate::api) struct SessionRequestError {
    pub transport: TransportError,
    started: bool,
}

impl SessionRequestError {
    pub(in crate::api) fn needs_reconciliation(&self) -> bool {
        self.started
            && match self.transport {
                TransportError::Network(Some(status @ 400..=499)) => matches!(status, 401 | 409),
                _ => true,
            }
    }
}

pub(in crate::api) async fn send_session_request(
    request: SessionRequest<'_>,
) -> Result<crate::api::clock::HttpResponse, SessionRequestError> {
    let mut started = false;
    #[cfg(test)]
    let fixture = dioxus::prelude::try_consume_context::<SessionHttpFixture>().map(|fixture| {
        started = true;
        (fixture.respond)(request)
    });
    send_response(
        |abort_signal| {
            let request = match request {
                SessionRequest::Read => Request::get(PATH)
                    .abort_signal(abort_signal)
                    .cache(web_sys::RequestCache::NoStore)
                    .build(),
                SessionRequest::Sync(user_id) => Request::put(PATH)
                    .abort_signal(abort_signal)
                    .header(PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
                    .build(),
                SessionRequest::Login(key) => Request::post(PATH)
                    .abort_signal(abort_signal)
                    .json(&PublicLoginRequest { key: key.to_owned() }),
                SessionRequest::Logout(user_id) => Request::delete(PATH)
                    .abort_signal(abort_signal)
                    .header(PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
                    .build(),
                SessionRequest::ReadPreference => {
                    Request::get(SESSION_PREFERENCE_PATH).abort_signal(abort_signal).build()
                }
                SessionRequest::SetPreference(user_id, command) => {
                    Request::put(SESSION_PREFERENCE_PATH)
                        .abort_signal(abort_signal)
                        .header(PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
                        .json(command)
                }
            };
            started = request.is_ok();
            request.map_err(|_| TransportError::Network(None))
        },
        #[cfg(test)]
        fixture,
    )
    .await
    .map_err(|transport| SessionRequestError { transport, started })
}

/// 权威身份只来自 HTTP 200 / `ok` 的对象信封，匿名也必须显式提供 `data`。
pub(in crate::api) fn decode_identity_response<T: DeserializeOwned>(
    status: u16,
    body: Option<&str>,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, PublicSessionApiError> {
    let invalid_response = || {
        report_browser_error(BrowserErrorClass::DtoDecode);
        PublicSessionApiError {
            status: Some(status),
            ..PublicSessionApiError::local("invalid_response")
        }
    };
    let body = body.ok_or_else(invalid_response)?;
    if !(200..300).contains(&status) {
        let (code, data) = decode_error_body(body).ok_or_else(invalid_response)?;
        let public_reason = (status == 403 && code == "account_blacklisted")
            .then(|| blacklist_reason(&data))
            .flatten();
        return Err(PublicSessionApiError {
            status: Some(status),
            code: Some(code),
            public_reason,
        });
    }
    let envelope = serde_json::from_str::<Value>(body)
        .ok()
        .filter(|value| value.as_object().is_some_and(|object| object.contains_key("data")))
        .ok_or_else(invalid_response)?;
    let envelope: ApiResponse<Value> =
        serde_json::from_value(envelope).map_err(|_decode_error| invalid_response())?;
    if status != 200 || envelope.code != "ok" || !validate(&envelope.data) {
        return Err(invalid_response());
    }
    serde_json::from_value(envelope.data).map_err(|_decode_error| invalid_response())
}

/// 私有响应若携带当前用户标识，必须通过与会话响应相同的信封检查。
pub(in crate::api) async fn decode_account_identity_response<T: DeserializeOwned>(
    response: impl Into<HttpResponse>,
    expected_user: Option<&str>,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, CommandError> {
    let response = response.into();
    let status = response.status();
    let body = response.text().await;
    decode_account_identity_body(status, body.as_deref(), expected_user, validate)
}

/// 锁外私有读取的成功与失败都须经过发起代次检查，旧失败也不能触发身份复核。
pub(in crate::api) async fn decode_observed_account_read<T: DeserializeOwned>(
    response: impl Into<HttpResponse>,
    current: impl FnOnce() -> bool,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, CommandError> {
    let response = response.into();
    let status = response.status();
    let body = response.text().await;
    if !current() {
        return Err(CommandError { code: "session_observation_stale".to_owned(), status: 0 });
    }
    decode_account_identity_body(status, body.as_deref(), None, validate)
}

fn decode_account_identity_body<T: DeserializeOwned>(
    status: u16,
    body: Option<&str>,
    expected_user: Option<&str>,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, CommandError> {
    decode_identity_response(status, body, validate).map_err(|error| {
        let code = error.into_read_code();
        observe_public_session_failure(status, &code, expected_user);
        CommandError { status, code }
    })
}

fn blacklist_reason(data: &Value) -> Option<String> {
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
}
