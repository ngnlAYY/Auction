//! 账号登录时长偏好的同源传输与严格解码；整数时长和版本不经 JavaScript 浮点转换。

use auction_contracts::{PublicSessionPreferenceResponse, PublicSetSessionPreferenceRequest};
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_has_exact_keys;
use serde_json::Value;

use super::CommandError;
use super::session::{
    PublicSessionApiError, SessionIdentity, SessionIdentityObservation, SessionRefreshReason,
    SessionRequest, broadcast_session_change, decode_identity_response, invalidate_cookie_sync,
    lock_cookie_mutation, observe_public_session_failure, publish_cookie_identity,
    request_public_session_check, send_session_request,
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
    if !observation.cookie_current() {
        return Err(failure("invalid_response", status));
    }
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
    ready: impl Fn() -> bool,
) -> Result<PublicSessionPreferenceResponse, CommandError> {
    if !ready() {
        request_public_session_check(SessionRefreshReason::CookieChanged);
        return Err(CommandError::before_send());
    }
    let queued = SessionIdentityObservation::start();
    // 成功、重放和未知结果的重发都须与登录共用原生锁，防止迟到 Set-Cookie 换回旧身份。
    let cookie_lock = lock_cookie_mutation().await.map_err(|code| failure(code, 0))?;
    if !queued.current() || !ready() {
        // 复核会申请同一把锁。本次未发送，先退出锁，调用方保留此前的未知命令。
        drop(cookie_lock);
        request_public_session_check(SessionRefreshReason::CookieChanged);
        return Err(CommandError::before_send());
    }
    invalidate_cookie_sync();
    let response = match send_session_request(SessionRequest::SetPreference(user_id, command)).await
    {
        Ok(response) => response,
        Err(error) => {
            if error.needs_reconciliation() {
                broadcast_session_change();
            }
            drop(cookie_lock);
            if error.needs_reconciliation() {
                request_public_session_check(SessionRefreshReason::CookieChanged);
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
    let recheck = (200..300).contains(&status)
        || status >= 500
        || (status == 409 && result.as_ref().is_err_and(|error| error.code == "invalid_response"));
    if recheck {
        // 先在锁内发布修订，再释放锁复核，避免排队的写入沿用旧 Cookie 代次。
        broadcast_session_change();
    }
    drop(cookie_lock);
    if recheck {
        request_public_session_check(SessionRefreshReason::CookieChanged);
    } else if let Err(error) = &result {
        observe_public_session_failure(status, &error.code, Some(user_id));
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

#[cfg(test)]
mod tests {
    #[test]
    fn session_preference_decodes_backend_contract_fixtures() {
        for (body, duration) in [
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/session-preference.json"
                ),
                None,
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/session-preference-timed.json"
                ),
                Some(3600),
            ),
        ] {
            let user = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
            let preference = super::decode(200, Some(body), user, None).expect("会话偏好");
            assert_eq!(preference.user_id, user);
            assert_eq!(preference.duration_seconds, duration);
        }
    }

    #[test]
    fn session_preference_errors_decode_backend_contract_fixtures() {
        use auction_contracts::PublicSetSessionPreferenceRequest;

        let user = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
        let command = PublicSetSessionPreferenceRequest {
            command_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            duration_seconds: Some(3600),
            expected_version: 5,
        };
        for (status, code, body) in [
            (
                409,
                "session_preference_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-session-preference-conflict.json"
                ),
            ),
            (
                422,
                "invalid_session_preference",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/error-invalid-session-preference.json"
                ),
            ),
        ] {
            let error = super::decode(status, Some(body), user, Some(&command))
                .expect_err("偏好命令已被拒绝");
            assert_eq!((error.code.as_str(), error.status), (code, status));
        }
    }
}
