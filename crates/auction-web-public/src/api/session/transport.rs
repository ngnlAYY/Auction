//! Public 会话请求及可用于身份证据的严格响应信封。

use auction_contracts::{ApiResponse, PUBLIC_EXPECTED_USER_ID_HEADER, PublicLoginRequest};
use auction_web_shared::{BrowserErrorClass, report_browser_error};
use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(test)]
use crate::api::clock::HttpFixtureFuture;
use crate::api::clock::send_response;
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
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct SessionHttpFixture(pub fn(SessionRequest<'_>) -> HttpFixtureFuture);

pub(super) async fn send_session_request(
    request: SessionRequest<'_>,
) -> Result<crate::api::clock::HttpResponse, PublicSessionApiError> {
    #[cfg(test)]
    let fixture = dioxus::prelude::try_consume_context::<SessionHttpFixture>()
        .map(|fixture| (fixture.0)(request));
    send_response(
        |abort_signal| {
            let request = match request {
                SessionRequest::Read => Request::get(PATH).abort_signal(abort_signal).build(),
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
            };
            request.map_err(|_| TransportError::Network(None))
        },
        #[cfg(test)]
        fixture,
    )
    .await
    .map_err(|error| PublicSessionApiError::local(error.code()))
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
    let envelope = body
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .filter(|value| value.as_object().is_some_and(|object| object.contains_key("data")))
        .ok_or_else(invalid_response)?;
    let envelope: ApiResponse<Value> =
        serde_json::from_value(envelope).map_err(|_decode_error| invalid_response())?;
    if !(200..300).contains(&status) {
        let public_reason = (status == 403 && envelope.code == "account_blacklisted")
            .then(|| blacklist_reason(&envelope.data))
            .flatten();
        return Err(PublicSessionApiError {
            status: Some(status),
            code: Some(envelope.code),
            public_reason,
        });
    }
    if status != 200 || envelope.code != "ok" || !validate(&envelope.data) {
        return Err(invalid_response());
    }
    serde_json::from_value(envelope.data).map_err(|_decode_error| invalid_response())
}

/// 私有响应若携带当前用户标识，必须通过与会话响应相同的信封检查。
pub(in crate::api) async fn decode_account_identity_response<T: DeserializeOwned>(
    response: Response,
    expected_user: Option<&str>,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, CommandError> {
    let status = response.status();
    let body = response.text().await.ok();
    decode_identity_response(status, body.as_deref(), validate).map_err(|error| {
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
