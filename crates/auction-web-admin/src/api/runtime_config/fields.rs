//! 运行配置 JSON 的字段读取、基础格式校验与响应错误构造。

use std::collections::HashSet;

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
    object.get(key)?.as_u64().filter(|value| *value <= MAX_JSON_SAFE_INTEGER as u64)
}

pub(super) fn valid_string_list(
    value: &Value,
    maximum_count: usize,
    maximum_bytes: usize,
    unique: bool,
) -> bool {
    let Some(values) = value.as_array() else {
        return false;
    };
    if values.len() > maximum_count {
        return false;
    }
    // 借用名称建立索引，不复制配置正文；唯一性校验按索引一次完成，无需逐项重扫前缀。
    let mut seen = HashSet::new();
    values.iter().all(|value| {
        value.as_str().is_some_and(|text| {
            valid_text(text, maximum_bytes, false) && (!unique || seen.insert(text))
        })
    })
}

pub(super) fn valid_text(value: &str, maximum_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= maximum_bytes
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.starts_with('/')
        && value != "/"
        && value == value.trim()
        && !value.chars().any(char::is_control)
        && !value.contains("//")
        && value.split('/').all(|segment| segment != "." && segment != "..")
}

pub(super) fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn invalid_runtime_response(_error: &'static str) -> AdminRuntimeConfigApiError {
    AdminApiFailure::client("invalid_response")
}
