//! 账号登录时长偏好的同源传输与严格解码；整数时长和版本不经 JavaScript 浮点转换。

use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_has_exact_keys;
use serde_json::Value;

use super::CommandError;
use super::session::{
    PublicSessionApiError, SessionIdentity, SessionIdentityObservation, SessionRefreshReason,
    SessionRequest, decode_identity_response, invalidate_cookie_sync, lock_cookie_mutation,
    notify_public_session_change, observe_public_session_failure, publish_cookie_identity,
    send_session_request,
};

pub(super) const PATH: &str = "/api/v1/me/session-preference";

/// 永久不设固定截止；有限偏好限制为整分钟，范围为一分钟到 365 天。
pub(crate) fn valid_session_duration(duration_seconds: Option<i64>) -> bool {
    duration_seconds.is_none_or(|seconds| (60..=31_536_000).contains(&seconds) && seconds % 60 == 0)
}

/// 读取账号偏好和当前 Cookie 的实际截止；两者允许因旧会话或其他设备保存而不同。
pub(crate) async fn fetch_session_preference(
    user_id: &str,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    let observation = SessionIdentityObservation::start();
    let response = send_session_request(SessionRequest::ReadPreference)
        .await
        .map_err(|error| failure(error.transport.code(), 0))?;
    let status = response.status();
    let body = response.text().await;
    let preference = decode(status, body.as_deref(), user_id, None).inspect_err(|error| {
        observe_public_session_failure(status, &error.code, None);
    })?;
    observation.publish(SessionIdentity::UserId(preference.user_id.clone()));
    if preference.user_id != user_id {
        return Err(failure("invalid_response", status));
    }
    Ok(preference)
}

/// 未知结果只重发原始命令及 CAS 版本；响应成功后通过外壳重读身份，不伪造登录结果。
pub(crate) async fn set_session_preference(
    user_id: &str,
    command: &PublicSetSessionPreferenceRequest,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    // 成功、重放和未知结果的重发都须与登录共用原生锁，防止迟到 Set-Cookie 换回旧身份。
    let cookie_lock = lock_cookie_mutation().await.map_err(|code| failure(code, 0))?;
    invalidate_cookie_sync();
    let response = match send_session_request(SessionRequest::SetPreference(user_id, command)).await
    {
        Ok(response) => response,
        Err(error) => {
            drop(cookie_lock);
            if error.needs_reconciliation() {
                notify_public_session_change(SessionRefreshReason::CookieChanged);
            }
            return Err(failure(error.transport.code(), 0));
        }
    };
    let status = response.status();
    let body = response.text().await;
    let result = decode(status, body.as_deref(), user_id, Some(command));
    if let Ok(preference) = &result {
        publish_cookie_identity(SessionIdentity::UserId(preference.user_id.clone()));
    }
    if (200..300).contains(&status) {
        // 完整 2xx 在锁内广播。接收方取得同一把锁后才 GET，且解码失败仍须复核。
        notify_public_session_change(SessionRefreshReason::CookieChanged);
    } else {
        drop(cookie_lock);
        if status >= 500
            || (status == 409
                && result.as_ref().is_err_and(|error| error.code == "invalid_response"))
        {
            // 无法确认提交时只读回 Cookie；偏好命令自己的冻结 ID 与 CAS 版本仍由表单保留。
            notify_public_session_change(SessionRefreshReason::CookieChanged);
        } else if let Err(error) = &result {
            observe_public_session_failure(status, &error.code, Some(user_id));
        }
    }
    result
}

fn decode(
    status: u16,
    body: Option<&str>,
    user_id: &str,
    command: Option<&PublicSetSessionPreferenceRequest>,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    let result: PublicSessionPreferenceResponse = decode_identity_response(status, body, |value| {
        json_has_exact_keys(value, &["user_id", "duration_seconds", "version", "expires_at"])
            && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
            && value
                .get("duration_seconds")
                .is_some_and(|value| value.is_null() || value.as_i64().is_some())
    })
    .map_err(|error: PublicSessionApiError| {
        let error = if command.is_some() {
            error.validate_conflict(&["session_changed", "session_preference_conflict"])
        } else {
            error
        };
        CommandError { code: error.into_read_code(), status }
    })?;
    if !valid_uuid(&result.user_id)
        || !valid_session_duration(result.duration_seconds)
        || !result
            .expires_at
            .as_deref()
            .is_none_or(|value| auction_time::Timestamp::parse_from_rfc3339(value).is_ok())
        || command.is_some_and(|command| {
            result.user_id != user_id
                || result.duration_seconds != command.duration_seconds
                || Some(result.version) != command.expected_version.checked_add(1)
                || result.expires_at.is_none() != command.duration_seconds.is_none()
        })
    {
        return Err(failure("invalid_response", status));
    }
    Ok(result)
}

fn failure(code: &str, status: u16) -> CommandError {
    CommandError { code: code.to_owned(), status }
}
