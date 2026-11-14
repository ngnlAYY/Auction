//! Public 成功响应的 route-specific decoder。
//! 旧版 Public 浏览器在共享 envelope 之后，仍对每条 route 的 `data` 使用 `json_keys_only(...)` 和
//! 跨字段谓词校验。本模块保留该边界：先验证 envelope 和原始 JSON，再反序列化为 contracts
//! DTO，避免 `serde` 的默认宽松行为把缺字段、未知字段或自相矛盾的投影送进组件。
//! 按路由分组的 decoder 位于子模块：`realtime`（实时事件流）、`catalog`（公开目录）、
//! `lot`（lot 目录/详情与竞价命令）、`account`（我的竞价与代理授权）；本文件只保留共享
//! envelope 解码与基础谓词原语。

use auction_web_shared::wire::{JSON_MAX_SAFE_INTEGER, JSON_MAX_SAFE_INTEGER_F64};
use auction_web_shared::{BrowserErrorClass, report_browser_error, valid_uuid};
use gloo_net::http::Response;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// 严格键集合谓词由 `auction-web-shared` 统一提供，子模块继续按旧名调用。
pub(crate) use auction_web_shared::wire::json_keys_only;

#[cfg(not(target_arch = "wasm32"))]
use chrono::DateTime;

mod account;
mod build_hello;
mod catalog;
mod lot;
#[cfg(target_arch = "wasm32")]
mod realtime;

pub(crate) use account::{decode_my_bids, decode_my_proxy};
#[cfg(target_arch = "wasm32")]
pub(crate) use build_hello::decode_build_hello;
pub(crate) use catalog::{
    decode_auction_list, decode_auction_list_v2, decode_bid_log_page, decode_blacklist,
    decode_compensation_lots, decode_compensation_pool, decode_current_auction,
    decode_price_history_page, decode_rules, decode_worlds,
};
pub(crate) use lot::{
    decode_bid_command, decode_create_proxy_command, decode_lot_detail, decode_lot_list,
    decode_lower_proxy_command,
};
#[cfg(target_arch = "wasm32")]
pub(crate) use realtime::{
    decode_auction_realtime_page, decode_discovery_realtime_page, decode_realtime_event,
};

/// 解析成功 envelope，并在反序列化前执行 route-specific data 校验。
pub(crate) async fn decode_success<T, F>(response: Response, validate: F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnOnce(&Value) -> bool,
{
    let payload = response.json::<Value>().await.map_err(|_| {
        report_browser_error(BrowserErrorClass::DtoDecode);
        "invalid_response".to_owned()
    })?;
    let decoded = decode_success_value(payload, validate);
    if decoded.is_err() {
        report_browser_error(BrowserErrorClass::DtoDecode);
    }
    decoded
}

/// 解析旧共享 client 接受的错误 envelope；顶层扩展允许存在，但三项基础字段必须显式出现。
pub(crate) async fn decode_error(response: Response) -> Result<(String, Value), String> {
    decode_error_value(response)
        .await
        .inspect_err(|_| report_browser_error(BrowserErrorClass::DtoDecode))
}

async fn decode_error_value(response: Response) -> Result<(String, Value), String> {
    let mut payload = response.json::<Value>().await.map_err(|_| "invalid_response".to_owned())?;
    let envelope = payload.as_object_mut().ok_or_else(|| "invalid_response".to_owned())?;
    let code =
        string(envelope.get("code")).ok_or_else(|| "invalid_response".to_owned())?.to_owned();
    string(envelope.get("message")).ok_or_else(|| "invalid_response".to_owned())?;
    let data = envelope.remove("data").ok_or_else(|| "invalid_response".to_owned())?;
    Ok((code, data))
}

/// 错误 envelope 无效时统一返回旧 client 的 `invalid_response`。
pub(crate) async fn decode_error_code(response: Response) -> String {
    decode_error(response).await.map_or_else(|code| code, |(code, _)| code)
}

fn decode_success_value<T, F>(mut payload: Value, validate: F) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnOnce(&Value) -> bool,
{
    let envelope = payload.as_object_mut().ok_or_else(|| "invalid_response".to_owned())?;
    if !string(envelope.get("code")).is_some()
        || !string(envelope.get("message")).is_some()
        || !envelope.contains_key("data")
    {
        return Err("invalid_response".to_owned());
    }
    // 信封已归本次解码所有，直接移出 data；仍先校验原始值，再规范化兼容整数。
    let mut data = envelope.remove("data").ok_or_else(|| "invalid_response".to_owned())?;
    if !validate(&data) {
        return Err("invalid_response".to_owned());
    }
    normalize_legacy_integer_numbers(&mut data);
    serde_json::from_value(data).map_err(|_| "invalid_response".to_owned())
}

fn normalize_legacy_integer_numbers(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(normalize_legacy_integer_numbers),
        Value::Object(values) => {
            values.values_mut().for_each(normalize_legacy_integer_numbers);
        }
        Value::Number(number) if number.as_i64().is_none() && number.as_u64().is_none() => {
            let Some(raw) = number.as_f64() else { return };
            if raw.is_finite() && raw.fract() == 0.0 && raw.abs() <= JSON_MAX_SAFE_INTEGER_F64 {
                *value = if raw.is_sign_negative() {
                    Value::Number(serde_json::Number::from(raw as i64))
                } else {
                    Value::Number(serde_json::Number::from(raw as u64))
                };
            }
        }
        _ => {}
    }
}

fn optional_nonempty(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || nonempty(value.get(key))
}

fn optional_nonnegative(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || nonnegative(value.get(key))
}

fn optional_time(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || time(value.get(key))
}

fn optional_finite(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || finite(value.get(key))
}

fn optional_primary_attribute_initials(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || string(value.get(key)).is_some_and(valid_primary_attribute_initials)
}

fn optional_bool(value: &Map<String, Value>, key: &str) -> bool {
    value.get(key).is_none() || bool_value(value.get(key)).is_some()
}

fn optional_string<'a>(value: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|value| string(Some(value)))
}

fn optional_int(value: &Map<String, Value>, key: &str) -> Option<i64> {
    value.get(key).and_then(|value| int(Some(value)))
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn string(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn int(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(|value| {
            let number = value.as_number()?;
            if let Some(integer) = number.as_i64() {
                return Some(integer);
            }
            let float = number.as_f64()?;
            (float.is_finite()
                && float.fract() == 0.0
                && (-JSON_MAX_SAFE_INTEGER_F64..=JSON_MAX_SAFE_INTEGER_F64).contains(&float))
            .then_some(float as i64)
        })
        .filter(|value| (-JSON_MAX_SAFE_INTEGER..=JSON_MAX_SAFE_INTEGER).contains(value))
}

fn positive(value: Option<&Value>) -> bool {
    int(value).is_some_and(|value| value > 0)
}

/// 分页数量单独接受完整正 i64；JSON 整数直接解码，金额及版本仍守旧安全整数边界。
/// 旧响应中的安全范围整数浮点写法继续兼容，不接受已丢失精度的大浮点数。
fn positive_page_size(value: Option<&Value>) -> bool {
    value.and_then(Value::as_i64).or_else(|| int(value)).is_some_and(|value| value > 0)
}

fn nonnegative(value: Option<&Value>) -> bool {
    int(value).is_some_and(|value| value >= 0)
}

fn revision(value: Option<&Value>) -> bool {
    nonnegative(value)
}

fn bool_value(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

fn finite(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64).is_some_and(f64::is_finite)
}

fn nonempty(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty() && value == value.trim())
}

fn account_nonempty(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty())
}

fn nonempty_mode(value: Option<&Value>, trimmed: bool) -> bool {
    if trimmed { nonempty(value) } else { account_nonempty(value) }
}

fn trimmed_nonempty_str(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn raw_nonempty_str(value: &str) -> bool {
    !value.is_empty()
}

#[cfg(target_arch = "wasm32")]
fn time(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| !value.is_empty() && !js_sys::Date::parse(value).is_nan())
}

#[cfg(not(target_arch = "wasm32"))]
fn time(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok())
}

#[cfg(target_arch = "wasm32")]
fn time_value(value: Option<&Value>) -> Option<f64> {
    string(value).and_then(|value| {
        let timestamp = js_sys::Date::parse(value);
        (!timestamp.is_nan()).then_some(timestamp)
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn time_value(value: Option<&Value>) -> Option<f64> {
    string(value)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis() as f64)
}

#[cfg(target_arch = "wasm32")]
fn world(value: Option<&Value>) -> bool {
    string(value).is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value == value.trim()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    })
}

fn proxy_mode(value: Option<&Value>) -> bool {
    matches!(string(value), Some("fast" | "slow"))
}

fn proxy_status(value: Option<&Value>) -> bool {
    matches!(string(value), Some("active" | "exhausted" | "frozen" | "closed"))
}

fn valid_primary_attribute_initials(value: &str) -> bool {
    let mut cursor = 0;
    let ordered = b"SDAEIW";
    !value.is_empty()
        && value.bytes().all(|byte| {
            let Some(position) = ordered[cursor..].iter().position(|candidate| *candidate == byte)
            else {
                return false;
            };
            cursor += position + 1;
            true
        })
}

fn forum_post_url(value: &str) -> bool {
    const PREFIX: &str = "https://forums.e-hentai.org/index.php?showtopic=";
    let Some(rest) = value.strip_prefix(PREFIX) else { return false };
    let Some((topic_and_post, entry_id)) = rest.split_once("#entry") else { return false };
    let Some((thread_id, post_id)) = topic_and_post.split_once("&p=") else { return false };
    canonical_forum_id(thread_id) && canonical_forum_id(post_id) && post_id == entry_id
}

fn canonical_forum_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 服务端必须交付真实 ID/key URL；仅为历史投影兼容保留空字符串解码。
fn game_url(value: Option<&Value>) -> bool {
    let Some(value) = string(value) else { return false };
    if value.is_empty() {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else { return false };
    let Some(path) = value
        .strip_prefix("https://hentaiverse.org/equip/")
        .or_else(|| value.strip_prefix("https://hentaiverse.org/isekai/equip/"))
    else {
        return false;
    };
    let mut parts = path.split('/');
    let equipment_id = parts.next().unwrap_or_default();
    let key = parts.next().unwrap_or_default();
    url.scheme() == "https"
        && url.host_str() == Some("hentaiverse.org")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !equipment_id.is_empty()
        && equipment_id.len() <= 32
        && equipment_id.bytes().all(|byte| byte.is_ascii_digit())
        && key.len() == 10
        && key.bytes().all(|byte| byte.is_ascii_hexdigit())
        && parts.next().is_none()
}
