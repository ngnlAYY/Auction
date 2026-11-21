//! Admin 浏览器端的 wire 校验助手：字段存在性、稳定 code、文本/时间边界与安全整数上限。
//! 这里的判断全部按 fail-closed 处理；页面只消费后端安全投影，不借此放宽服务端规则。

use auction_time::Timestamp;
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

    cfg_select! {
        target_arch = "wasm32" => {
            let timestamp = js_sys::Date::parse(value);
            timestamp.is_finite().then_some(timestamp)
        }
        _ => auction_time::Timestamp::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.timestamp_millis() as f64),
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

/// 本地钟点在当前时区中无法唯一对应 UTC 的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalTimeError {
    Invalid,
    Nonexistent,
    Ambiguous,
}

/// 只接受在浏览器时区中有唯一 UTC 对应的本地日期时间。
pub(crate) fn parse_local_datetime(value: &str) -> Result<Timestamp, LocalTimeError> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new_0();
        resolve_local_datetime(value, |utc_millis| {
            if !date.set_time(utc_millis as f64).is_finite() {
                return None;
            }
            Some((
                [
                    date.get_full_year() as i32,
                    date.get_month() as i32 + 1,
                    date.get_date() as i32,
                    date.get_hours() as i32,
                    date.get_minutes() as i32,
                ],
                date.get_seconds(),
            ))
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    resolve_local_datetime(value, |utc_millis| {
        let utc = Timestamp::from_timestamp_millis(utc_millis)?;
        Some(local_fields(utc))
    })
}

fn resolve_local_datetime(
    value: &str,
    mut local_at: impl FnMut(i64) -> Option<([i32; 5], u32)>,
) -> Result<Timestamp, LocalTimeError> {
    // 用 UTC 文本表示本地字段以严格校验日期；真正的 UTC 时刻由反向核对确定。
    let wall = if value.len() == 16 { format!("{value}:00Z") } else { format!("{value}Z") };
    let wall = Timestamp::parse_from_rfc3339(&wall).map_err(|_error| LocalTimeError::Invalid)?;
    let (target, target_second) = local_fields(wall);
    let Some(wall_minute_millis) = wall
        .timestamp()
        .checked_sub(i64::from(target_second))
        .and_then(|seconds| seconds.checked_mul(1_000))
    else {
        return Err(LocalTimeError::Invalid);
    };
    let mut utc = None;
    // 时区偏移不足一天。逐分钟反查，并补正含秒的旧偏移，不把时间移到缺口之后或择一回拨时刻。
    for minutes in -1_440_i64..=1_440 {
        let Some(probe) = wall_minute_millis.checked_add(minutes * 60_000) else {
            continue;
        };
        let Some((fields, second)) = local_at(probe) else { continue };
        if fields != target {
            continue;
        }
        let Some(candidate) = i64::from(target_second)
            .checked_sub(i64::from(second))
            .and_then(|delta| delta.checked_mul(1_000))
            .and_then(|delta| probe.checked_add(delta))
        else {
            continue;
        };
        if target_second != second && local_at(candidate) != Some((target, target_second)) {
            continue;
        }
        if utc.replace(candidate).is_some_and(|previous| previous != candidate) {
            return Err(LocalTimeError::Ambiguous);
        }
    }
    utc.and_then(|millis| Timestamp::from_timestamp(millis / 1_000, wall.timestamp_subsec_nanos()))
        .ok_or(LocalTimeError::Nonexistent)
}

fn local_fields(timestamp: Timestamp) -> ([i32; 5], u32) {
    let fields = timestamp.to_utc_datetime();
    (
        [
            i32::from(fields.year()),
            i32::from(fields.month()),
            i32::from(fields.day()),
            i32::from(fields.hour()),
            i32::from(fields.minute()),
        ],
        u32::from(fields.second().unsigned_abs()),
    )
}

/// API 的 RFC3339 时间 → 浏览器 `datetime-local` 的本地
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
    use auction_time::Timestamp;

    use super::{
        LocalTimeError, datetime_local_value, local_fields, parse_local_datetime,
        resolve_local_datetime,
    };

    fn utc(value: &str) -> Timestamp {
        Timestamp::parse_from_rfc3339(value).unwrap()
    }

    fn local_at_offset(probe: i64, offset_minutes: i64) -> Option<([i32; 5], u32)> {
        let local = Timestamp::from_timestamp_millis(probe + offset_minutes * 60_000)?;
        Some(local_fields(local))
    }

    #[test]
    fn local_minute_round_trips_through_utc() {
        // 宿主构建的“本地时区”退化为 UTC；WASM 构建两端都按浏览器时区换算。
        assert_eq!(
            datetime_local_value("2026-11-14T09:05:00Z").as_deref(),
            Some("2026-11-14T09:05")
        );
        let instant = parse_local_datetime("2026-11-14T09:05").unwrap();
        assert_eq!(instant, utc("2026-11-14T09:05:00Z"));
        let local = datetime_local_value(&instant.to_rfc3339());
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
        assert_eq!(parse_local_datetime(""), Err(LocalTimeError::Invalid));
    }

    #[test]
    fn spring_forward_accepts_a_valid_time_and_rejects_a_skipped_one() {
        let transition = utc("2027-03-14T10:00:00Z").timestamp_millis();
        let local_at = |probe| local_at_offset(probe, if probe < transition { -480 } else { -420 });
        assert_eq!(
            resolve_local_datetime("2027-03-14T03:30", local_at),
            Ok(utc("2027-03-14T10:30:00Z"))
        );
        assert_eq!(
            resolve_local_datetime("2027-03-14T03:30:19.123456789", local_at),
            Ok(utc("2027-03-14T10:30:19.123456789Z"))
        );
        assert_eq!(
            resolve_local_datetime("2027-03-14T02:30", local_at),
            Err(LocalTimeError::Nonexistent)
        );
        assert_eq!(
            resolve_local_datetime("2027-02-29T12:00", local_at),
            Err(LocalTimeError::Invalid)
        );
    }

    #[test]
    fn rejects_repeated_times_without_assuming_a_one_hour_shift() {
        let transition = utc("2027-11-07T09:00:00Z").timestamp_millis();
        for shift in [30, 60] {
            let local_at = |probe| {
                local_at_offset(probe, if probe < transition { -420 } else { -420 - shift })
            };
            assert_eq!(
                resolve_local_datetime("2027-11-07T01:45", local_at),
                Err(LocalTimeError::Ambiguous)
            );
            assert_eq!(
                resolve_local_datetime("2027-11-07T02:00", local_at),
                Ok(utc(if shift == 30 { "2027-11-07T09:30:00Z" } else { "2027-11-07T10:00:00Z" }))
            );
        }
    }
}
