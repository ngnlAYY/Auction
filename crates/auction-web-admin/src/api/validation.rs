//! Admin 浏览器端的 wire 校验助手：字段存在性、稳定 code、文本/时间边界与安全整数上限。
//! 这里的判断全部按 fail-closed 处理；页面只消费后端安全投影，不借此放宽服务端规则。

use serde::Deserialize;

/// JSON 能无损表达的最大整数；所有从 wire 读取的计数、金额与版本都不得超过它。
pub(crate) const MAX_JSON_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// 成功 envelope 的稳定 `code` 闭集；投递路由与论坛同步状态共用。
pub(crate) fn valid_success_code(code: &str) -> bool {
    matches!(code, "ok" | "created" | "accepted")
}

/// 让 JSON `null` 可映射为 `None`，同时仍要求响应中显式存在该字段。
#[derive(Debug)]
pub(crate) struct RequiredOption<T>(pub(crate) Option<T>);

impl<'de, T> Deserialize<'de> for RequiredOption<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self)
    }
}

/// 表示可以缺失、但一旦出现就不能是 JSON `null` 的旧 wire 字段。
#[derive(Debug)]
pub(crate) struct OptionalField<T>(pub(crate) Option<T>);

impl<T> Default for OptionalField<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T> Deserialize<'de> for OptionalField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(|value| Self(Some(value)))
    }
}

/// 对齐旧浏览器 `Date.parse` 的时间接受边界；非 WASM 分支只用于本地静态编译。
pub(crate) fn parse_admin_time(value: &str) -> Option<f64> {
    if value.is_empty() {
        return None;
    }

    #[cfg(target_arch = "wasm32")]
    {
        let timestamp = js_sys::Date::parse(value);
        timestamp.is_finite().then_some(timestamp)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.timestamp_millis() as f64)
    }
}

pub(crate) fn valid_admin_time(value: &str) -> bool {
    parse_admin_time(value).is_some()
}

pub(crate) fn valid_trimmed_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

pub(crate) fn valid_safe_text(value: &str, maximum_bytes: usize) -> bool {
    valid_trimmed_text(value)
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

/// 将浏览器 `datetime-local` 按本地时区转换为 API 使用的 UTC 文本。
/// Admin 的创建和筛选必须共享这一转换，否则非 UTC 管理员提交的时间范围会产生偏移。
pub(crate) fn normalize_datetime(value: &str) -> String {
    if value.is_empty()
        || value.ends_with('Z')
        || value.contains('+')
        || value.get(10..).is_some_and(|tail| tail.contains('-'))
    {
        return value.to_owned();
    }

    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        if date.get_time().is_finite() {
            return date.to_iso_string().into();
        }
    }

    // 非 WASM 构建只用于静态检查；保留服务端可识别的稳定兜底格式。
    if value.len() == 16 { format!("{value}:00Z") } else { format!("{value}Z") }
}
