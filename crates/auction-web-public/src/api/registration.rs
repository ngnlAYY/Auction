//! 登录页注册账号的匿名读取；只接收公开名单，不读取浏览器外的配置文件。

use auction_contracts::PublicRegistrationInfoResponse;
use serde_json::Value;

use super::{decoder, send_get};

/// 沿用同源请求总期限；失败不能用示例账号替代配置。
pub(crate) async fn fetch_registration_info() -> Result<PublicRegistrationInfoResponse, String> {
    let response =
        send_get("/api/v1/registration-info").await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_success(response, valid_registration_info).await
}

fn valid_registration_info(value: &Value) -> bool {
    let Some(object) = auction_web_shared::wire::json_exact_object(value, &["bot_players"]) else {
        return false;
    };
    let Some(players) = object.get("bot_players").and_then(Value::as_array) else {
        return false;
    };
    players.len() <= 16
        && players.iter().enumerate().all(|(index, value)| {
            value.as_str().is_some_and(|name| {
                !name.is_empty()
                    && name.len() <= 64
                    && name.trim() == name
                    && !name.chars().any(char::is_control)
                    && !players[..index].iter().any(|previous| {
                        previous
                            .as_str()
                            .is_some_and(|previous| previous.eq_ignore_ascii_case(name))
                    })
            })
        })
}
