//! 账号竞价警告偏好的同源传输；完整 i64 版本不经过 JavaScript 数字转换。

use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use auction_web_shared::valid_uuid;
use auction_web_shared::wire::json_has_exact_keys;
use gloo_net::http::Request;
use serde_json::Value;

use super::clock::{HttpResponse, send_response};
use super::session::{
    SessionIdentity, SessionIdentityObservation, decode_account_identity_response,
};
use super::{CommandError, TransportError};

const PATH: &str = "/api/v1/me/lot-watch-preference";
/// 权威响应必须恰好包含契约字段；漏掉 `pm_available` 会让整页退化为“无法读取偏好”。
const RESPONSE_KEYS: [&str; 4] = ["user_id", "channel", "version", "pm_available"];

/// 渠道偏好的 wire 闭集（`none`/`mm`/`pm`/`both`）与两个独立开关的互换；闭集之外返回 `None`。
/// 与后端 `LotWatchChannels` 同一张表：浏览器只做展示映射，不自行扩展取值。
pub(crate) fn channel_flags(value: &str) -> Option<(bool, bool)> {
    match value {
        "none" => Some((false, false)),
        "mm" => Some((true, false)),
        "pm" => Some((false, true)),
        "both" => Some((true, true)),
        _ => None,
    }
}

/// 两个独立开关对应的 wire 值；与 [`channel_flags`] 互逆。
pub(crate) fn channel_value(mm: bool, pm: bool) -> &'static str {
    match (mm, pm) {
        (false, false) => "none",
        (true, false) => "mm",
        (false, true) => "pm",
        (true, true) => "both",
    }
}

/// 只接受当前会话身份的完整权威投影，不推导默认偏好。
pub(crate) async fn fetch_lot_watch_preference(
    user_id: &str,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let observation = SessionIdentityObservation::start();
    let response = send_response(
        |abort_signal| {
            Request::get(PATH)
                .abort_signal(abort_signal)
                .build()
                .map_err(|_error| TransportError::Network(None))
        },
        #[cfg(test)]
        crate::surface::PageReadFixture::response((PATH, user_id.to_owned())),
    )
    .await
    .map_err(|error| failure(error.code(), 0))?;
    decode(response, user_id, None, observation).await
}

/// 未知结果重试必须传入原 DTO，不能改写命令 ID、渠道或 CAS 版本。
pub(crate) async fn set_lot_watch_preference(
    user_id: &str,
    command: &PublicSetLotWatchPreferenceRequest,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let observation = SessionIdentityObservation::start();
    let response = send_response(
        |abort_signal| {
            Request::put(PATH)
                .abort_signal(abort_signal)
                .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
                .json(command)
                .map_err(|_| TransportError::Network(None))
        },
        #[cfg(test)]
        crate::surface::PageReadFixture::response((PATH, user_id.to_owned(), command.clone())),
    )
    .await
    .map_err(|error| failure(error.code(), 0))?;
    decode(response, user_id, Some(command), observation).await
}

async fn decode(
    response: HttpResponse,
    user_id: &str,
    command: Option<&PublicSetLotWatchPreferenceRequest>,
    observation: SessionIdentityObservation,
) -> Result<PublicLotWatchPreferenceResponse, CommandError> {
    let status = response.status();
    let result: PublicLotWatchPreferenceResponse =
        decode_account_identity_response(response, command.map(|_| user_id), |value| {
            json_has_exact_keys(value, &RESPONSE_KEYS)
                && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
        })
        .await?;
    if !valid_uuid(&result.user_id)
        || channel_flags(&result.channel).is_none()
        || command.is_some_and(|command| {
            result.user_id != user_id
                || result.channel != command.channel
                || result.version <= command.expected_version
        })
    {
        return Err(failure("invalid_response", status));
    }
    observation.publish(SessionIdentity::UserId(result.user_id.clone()));
    if result.user_id != user_id {
        return Err(failure("invalid_response", status));
    }
    Ok(result)
}

fn failure(code: &str, status: u16) -> CommandError {
    CommandError { code: code.to_owned(), status }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::wire::json_has_exact_keys;
    use serde_json::json;

    use super::{RESPONSE_KEYS, channel_flags, channel_value};

    #[test]
    fn channel_flags_and_value_round_trip_the_closed_set() {
        for value in ["none", "mm", "pm", "both"] {
            let (mm, pm) = channel_flags(value).expect("closed set value");
            assert_eq!(channel_value(mm, pm), value);
        }
        assert_eq!(channel_flags(""), None);
        assert_eq!(channel_flags("MM"), None);
        assert_eq!(channel_flags("mm,pm"), None);
    }

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

    #[test]
    fn watch_preference_decodes_backend_contract_fixtures() {
        use auction_contracts::PublicLotWatchPreferenceResponse;

        for (body, channel, pm_available) in [
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/lot-watch-preference.json"
                ),
                "mm",
                false,
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/public/lot-watch-preference-both.json"
                ),
                "both",
                true,
            ),
        ] {
            let preference: PublicLotWatchPreferenceResponse =
                super::super::session::decode_identity_response(200, Some(body), |value| {
                    json_has_exact_keys(value, &RESPONSE_KEYS)
                        && value
                            .get("version")
                            .and_then(serde_json::Value::as_i64)
                            .is_some_and(|version| version > 0)
                })
                .expect("标记偏好");
            assert_eq!(preference.channel, channel);
            assert_eq!(preference.pm_available, pm_available);
            assert!(channel_flags(&preference.channel).is_some());
        }
    }
}
