//! Admin 浏览器端的 wire 校验助手：字段存在性、稳定 code、文本/时间边界与安全整数上限。
//! 这里的判断全部按 fail-closed 处理；页面只消费后端安全投影，不借此放宽服务端规则。

use serde::Deserialize;

/// JSON 能无损表达的最大整数；所有从 wire 读取的计数、金额与版本都不得超过它。
pub(crate) const MAX_JSON_SAFE_INTEGER: i64 = auction_web_shared::wire::JSON_MAX_SAFE_INTEGER;

/// 成功 envelope 的稳定 `code` 闭集；投递路由与论坛同步状态共用。
pub(crate) fn valid_success_code(code: &str) -> bool {
    matches!(code, "ok" | "created" | "accepted")
}

/// 表示可以缺失、但一旦出现就不能是 JSON `null` 的旧 wire 字段。
/// 反方向的“必须显式出现、可为 `null`”统一用
/// `#[serde(deserialize_with = "auction_contracts::deserialize_required_option")]`：
/// 只有 `deserialize_with` 字段在缺失时会让 serde 报错，包装类型做不到这一点。
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
        auction_time::Timestamp::parse_from_rfc3339(value)
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

/// 可空文本：缺省合法，出现时与 [`valid_trimmed_text`] 同一边界。
pub(crate) fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_trimmed_text)
}

/// 可空时间：缺省合法，出现时与 [`valid_admin_time`] 同一边界。
pub(crate) fn valid_optional_time(value: Option<&str>) -> bool {
    value.is_none_or(valid_admin_time)
}

/// 计数、金额等非负 JSON 安全整数。
pub(crate) fn valid_count(value: i64) -> bool {
    (0..=MAX_JSON_SAFE_INTEGER).contains(&value)
}

pub(crate) fn valid_safe_text(value: &str, maximum_bytes: usize) -> bool {
    valid_trimmed_text(value)
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

/// 将浏览器 `datetime-local` 按本地时区转换为 API 使用的 UTC 文本。
/// 保留发货证据与筛选的既有换算约定；场次排期单独拒绝不存在或重复的本地钟点。
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

/// [`normalize_datetime`] 的反向转换：API 的 RFC3339 时间 → 浏览器 `datetime-local` 的本地
/// 分钟文本（`YYYY-MM-DDTHH:MM`，不带时区）。预填输入框与提交换算必须共用同一时区规则；
/// 带偏移的文本会被 `datetime-local` 净化成空值。非 WASM 构建按 UTC 显示，只用于静态检查与测试。
pub(crate) fn datetime_local_value(value: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        if !date.get_time().is_finite() {
            return None;
        }
        Some(format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            date.get_full_year(),
            date.get_month() + 1,
            date.get_date(),
            date.get_hours(),
            date.get_minutes(),
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        auction_time::Timestamp::parse_from_rfc3339(value).ok().map(|value| {
            let utc = value.to_utc_datetime();
            format!("{}T{:02}:{:02}", utc.date(), utc.hour(), utc.minute())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{datetime_local_value, normalize_datetime};

    #[test]
    fn local_minute_round_trips_through_normalized_utc() {
        // 宿主构建的“本地时区”退化为 UTC；WASM 构建两端都按浏览器时区换算。
        assert_eq!(
            datetime_local_value("2026-11-14T09:05:00Z").as_deref(),
            Some("2026-11-14T09:05")
        );
        assert_eq!(normalize_datetime("2026-11-14T09:05"), "2026-11-14T09:05:00Z");
        let local = datetime_local_value(&normalize_datetime("2026-11-14T09:05"));
        assert_eq!(local.as_deref(), Some("2026-11-14T09:05"));
    }

    #[test]
    fn local_value_drops_seconds_and_rejects_unparseable_text() {
        assert_eq!(
            datetime_local_value("2026-11-14T09:05:37.250+08:00").as_deref(),
            Some("2026-11-14T01:05")
        );
        assert_eq!(datetime_local_value(""), None);
        assert_eq!(datetime_local_value("not a time"), None);
        assert_eq!(normalize_datetime(""), "");
    }
}
