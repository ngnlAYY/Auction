//! 账号竞价警告偏好的同源传输；完整 i64 版本不经过 JavaScript 数字转换。

use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use auction_web_shared::is_legacy_uuid;
use auction_web_shared::wire::json_has_exact_keys;
use gloo_net::http::Request;
use serde_json::Value;

use super::{CommandError, TransportError, decoder, send_get, send_request};

const PATH: &str = "/api/v1/me/lot-watch-preference";
/// 权威响应必须恰好包含契约字段；漏掉 `pm_available` 会让整页退化为“无法读取偏好”。
const RESPONSE_KEYS: [&str; 4] = ["user_id", "channel", "version", "pm_available"];

/// 只接受当前会话身份的完整权威投影，不推导默认偏好。
pub(crate) async fn fetch_lot_watch_preference(
    user_id: &str,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let response = send_get(PATH).await.map_err(|error| failure(error.code(), 0))?;
    decode(response, user_id, None).await
}

/// 未知结果重试必须传入原 DTO，不能改写命令 ID、渠道或 CAS 版本。
pub(crate) async fn set_lot_watch_preference(
    user_id: &str,
    command: &PublicSetLotWatchPreferenceRequest,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let response = send_request(|abort_signal| {
        Request::put(PATH)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
            .json(command)
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| failure(error.code(), 0))?;
    decode(response, user_id, Some(command)).await
}

async fn decode(
    response: gloo_net::http::Response,
    user_id: &str,
    command: Option<&PublicSetLotWatchPreferenceRequest>,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let status = response.status();
    if !response.ok() {
        return Err(failure(&decoder::decode_error_code(response).await, status));
    }
    let result: PublicLotWatchPreferenceResponse = decoder::decode_success(response, |value| {
        json_has_exact_keys(value, &RESPONSE_KEYS)
            && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
    })
    .await
    .map_err(|_| failure("invalid_response", status))?;
    if result.user_id != user_id
        || !is_legacy_uuid(&result.user_id)
        || !matches!(result.channel.as_str(), "mm" | "pm")
        || command.is_some_and(|command| {
            result.channel != command.channel || result.version <= command.expected_version
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

#[cfg(test)]
mod tests {
    use super::RESPONSE_KEYS;
    use auction_web_shared::wire::json_has_exact_keys;
    use serde_json::json;

    #[test]
    fn preference_response_requires_pm_available() {
        let full = json!({
            "user_id": "44f6d78b-ad02-447e-897f-f89731f98566",
            "channel": "mm",
            "version": 1,
            "pm_available": false,
        });
        assert!(json_has_exact_keys(&full, &RESPONSE_KEYS));
        let missing = json!({"user_id": "u", "channel": "mm", "version": 1});
        assert!(!json_has_exact_keys(&missing, &RESPONSE_KEYS));
    }
}
