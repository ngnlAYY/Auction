//! 浏览器 wire 格式的共享原语：JSON 安全整数边界与严格键集合谓词。
//! 两端 decoder 都以这些谓词拒绝缺字段、未知字段和已丢失精度的整数，业务规则仍留在各自模块。

use serde_json::{Map, Value};

/// JSON（IEEE-754 double）能无损表达的最大整数；金额、计数与版本都不得超过它。
pub const JSON_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// [`JSON_MAX_SAFE_INTEGER`] 的浮点形式，供 `serde_json` 的浮点写法比较使用。
pub const JSON_MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

/// 对象的键必须与 `keys` 完全一致：不接受缺失键，也不接受多余键。
pub fn json_exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

/// [`json_exact_keys`] 的值形式：不是对象或键集合不严格一致时为 `None`。
pub fn json_exact_object<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    json_exact_keys(object, keys).then_some(object)
}

/// [`json_exact_keys`] 的布尔值形式，便于直接作为 decoder 谓词。
pub fn json_has_exact_keys(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| json_exact_keys(object, keys))
}

/// 键集合闭包：`required` 必须全部出现，只允许 `required ∪ optional`，`optional` 可以缺省。
pub fn json_keys_only(object: &Map<String, Value>, required: &[&str], optional: &[&str]) -> bool {
    required.iter().all(|key| object.contains_key(*key))
        && object
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}
