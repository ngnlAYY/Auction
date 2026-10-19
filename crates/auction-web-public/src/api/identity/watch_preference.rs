//! 账号竞价警告偏好的同源传输；完整 i64 版本不经过 JavaScript 数字转换。

use auction_contracts::{PublicLotWatchPreferenceResponse, PublicSetLotWatchPreferenceRequest};
use gloo_net::http::Request;
use serde_json::Value;
use uuid::Uuid;

use super::{CommandError, TransportError, decoder, send_get, send_request};

const PATH: &str = "/api/v1/me/lot-watch-preference";

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
        value.as_object().is_some_and(|object| {
            object.len() == 3
                && ["user_id", "channel", "version"].iter().all(|key| object.contains_key(*key))
        }) && value.get("version").and_then(Value::as_i64).is_some_and(|version| version > 0)
    })
    .await
    .map_err(|_| failure("invalid_response", status))?;
    if result.user_id != user_id
        || !Uuid::parse_str(&result.user_id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == result.user_id)
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
