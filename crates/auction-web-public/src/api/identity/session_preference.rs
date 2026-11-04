//! 账号登录时长偏好的同源传输与严格解码；整数时长和版本不经 JavaScript 浮点转换。

use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use gloo_net::http::Request;
use serde_json::Value;
use uuid::Uuid;

use super::{CommandError, TransportError, decoder, send_get, send_request};

const PATH: &str = "/api/v1/me/session-preference";

/// 永久不设固定截止；有限偏好限制为整分钟，范围为一分钟到 365 天。
pub(crate) fn valid_session_duration(duration_seconds: Option<i64>) -> bool {
    duration_seconds.is_none_or(|seconds| (60..=31_536_000).contains(&seconds) && seconds % 60 == 0)
}

/// 读取账号偏好和当前 Cookie 的实际截止；两者允许因旧会话或其他设备保存而不同。
pub(crate) async fn fetch_session_preference(
    user_id: &str,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    let response = send_get(PATH).await.map_err(|error| failure(error.code(), 0))?;
    decode(response, user_id, None).await
}

/// 未知结果只重发原始命令及 CAS 版本；响应成功后通过外壳重读身份，不伪造登录结果。
pub(crate) async fn set_session_preference(
    user_id: &str,
    command: &PublicSetSessionPreferenceRequest,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    // 成功、重放和未知结果的重发都须与登录共用原生锁，防止迟到 Set-Cookie 换回旧身份。
    let _cookie_lock =
        super::session::lock_cookie_mutation().await.map_err(|code| failure(code, 0))?;
    super::session::invalidate_cookie_sync();
    let response = send_request(|abort_signal| {
        Request::put(PATH)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
            .json(command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| failure(error.code(), 0))?;
    if response.ok() {
        // 即使正文损坏，Cookie 和服务端截止也可能已经更新，仍须通知本页及其他标签页。
        super::session::notify_public_session_change(super::session::SessionRefreshReason::Refresh);
    }
    decode(response, user_id, Some(command)).await
}

async fn decode(
    response: gloo_net::http::Response,
    user_id: &str,
    command: Option<&PublicSetSessionPreferenceRequest>,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    let status = response.status();
    if !response.ok() {
        return Err(failure(&decoder::decode_error_code(response).await, status));
    }
    let result: PublicSessionPreferenceResponse = decoder::decode_success(response, |value| {
        value.as_object().is_some_and(|object| {
            object.len() == 4
                && ["user_id", "duration_seconds", "version", "expires_at"]
                    .iter()
                    .all(|key| object.contains_key(*key))
        }) && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
            && value
                .get("duration_seconds")
                .is_some_and(|value| value.is_null() || value.as_i64().is_some())
    })
    .await
    .map_err(|_| failure("invalid_response", status))?;
    if result.user_id != user_id
        || !Uuid::parse_str(&result.user_id).is_ok_and(|id| {
            let mut buffer = Uuid::encode_buffer();
            !id.is_nil() && id.hyphenated().encode_lower(&mut buffer) == result.user_id.as_str()
        })
        || !valid_session_duration(result.duration_seconds)
        || !result
            .expires_at
            .as_deref()
            .is_none_or(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
        || command.is_some_and(|command| {
            result.duration_seconds != command.duration_seconds
                || Some(result.version) != command.expected_version.checked_add(1)
                || result.expires_at.is_none() != command.duration_seconds.is_none()
        })
    {
        return Err(failure("invalid_response", status));
    }
    Ok(result)
}

fn failure(code: &str, status: u16) -> CommandError {
    super::session::observe_public_session_failure(status, code);
    CommandError { code: code.to_owned(), status }
}
