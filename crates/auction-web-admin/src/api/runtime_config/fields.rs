//! 运行配置 JSON 的结构校验、字段读取与响应错误构造。

use serde_json::{Map, Value};

use super::AdminRuntimeConfigApiError;
use crate::api::{AdminApiFailure, MAX_JSON_SAFE_INTEGER};

/// `bot_players` 等兼容字段可以缺省，但出现时仍必须是闭集内已知键。
pub(super) fn optional_keys_object<'a>(
    value: &'a Value,
    keys: &[&str],
) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    object.keys().all(|key| keys.contains(&key.as_str())).then_some(object)
}

pub(super) fn bool_field(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key)?.as_bool()
}

pub(super) fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

pub(super) fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    safe_u64(object.get(key)?)
}

pub(super) fn safe_u64(value: &Value) -> Option<u64> {
    value.as_u64().filter(|value| *value <= MAX_JSON_SAFE_INTEGER as u64)
}

pub(super) fn safe_i64(value: &Value) -> Option<i64> {
    value.as_i64().filter(|value| (-MAX_JSON_SAFE_INTEGER..=MAX_JSON_SAFE_INTEGER).contains(value))
}

pub(super) fn valid_string_array(value: &Value) -> bool {
    value.as_array().is_some_and(|values| values.iter().all(Value::is_string))
}

/// 响应只核对字段闭集、类型和安全整数，取值规则由服务端校验候选时裁定。
pub(super) fn valid_scalars(
    value: &Value,
    booleans: &[&str],
    strings: &[&str],
    numbers: &[&str],
    nested: &[&str],
) -> bool {
    valid_scalars_with_signed(value, booleans, strings, numbers, &[], nested)
}

/// 有符号字段可携带安全范围内的负数，业务范围留给表单和服务端。
pub(super) fn valid_scalars_with_signed(
    value: &Value,
    booleans: &[&str],
    strings: &[&str],
    unsigned: &[&str],
    signed: &[&str],
    nested: &[&str],
) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == booleans.len() + strings.len() + unsigned.len() + signed.len() + nested.len()
        && nested.iter().all(|name| object.contains_key(*name))
        && booleans.iter().all(|name| bool_field(object, name).is_some())
        && strings.iter().all(|name| string_field(object, name).is_some())
        && unsigned.iter().all(|name| u64_field(object, name).is_some())
        && signed.iter().all(|name| object.get(*name).and_then(safe_i64).is_some())
}

pub(super) fn valid_text(value: &str, maximum_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= maximum_bytes
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn invalid_runtime_response(_error: &'static str) -> AdminRuntimeConfigApiError {
    AdminApiFailure::client("invalid_response")
}
