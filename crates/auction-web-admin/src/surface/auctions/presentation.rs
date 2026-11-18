//! Admin 拍卖页面共享的状态、时间和文案映射。

use auction_time::Timestamp;
use auction_web_shared::t;

pub(super) fn admin_auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("admin-auctions-status-draft"),
        "preview" => t!("admin-auctions-status-preview"),
        "live" => t!("admin-auctions-status-live"),
        "final" => t!("admin-auctions-status-final"),
        "settling" => t!("admin-auctions-status-settling"),
        "settled" => t!("admin-auctions-status-settled"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
}

pub(super) fn current_time_millis() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::new_0().get_time() as i64
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        auction_time::Timestamp::now().timestamp_millis()
    }
}

pub(super) fn scheduled_time_millis(value: &str) -> Option<i64> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        let milliseconds = date.get_time();
        milliseconds.is_finite().then_some(milliseconds as i64)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        auction_time::Timestamp::parse_from_rfc3339(value)
            .ok()
            .map(auction_time::Timestamp::timestamp_millis)
    }
}

/// 候选排期使用已经解析的 UTC，供表单校验及首次提交后原样重试。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScheduleTimes {
    pub(super) preview: Timestamp,
    pub(super) open: Timestamp,
    pub(super) end: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScheduleField {
    Preview,
    Open,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScheduleIssue {
    Required,
    InvalidTime,
    NonexistentLocalTime,
    AmbiguousLocalTime,
    PreviewNotFuture,
    OpenBeforePreview,
    OpenNotFuture,
    EndNotAfterOpen,
}

impl ScheduleIssue {
    pub(super) fn message(self) -> String {
        match self {
            Self::Required => t!("admin-field-required"),
            Self::InvalidTime => t!("admin-auctions-invalid-local-time"),
            Self::NonexistentLocalTime => t!("admin-auctions-nonexistent-local-time"),
            Self::AmbiguousLocalTime => t!("admin-auctions-ambiguous-local-time"),
            Self::PreviewNotFuture => t!("admin-auctions-create-field-preview-past"),
            Self::OpenBeforePreview => t!("admin-auctions-create-field-open-before-preview"),
            Self::OpenNotFuture => t!("admin-auctions-schedule-field-open-past"),
            Self::EndNotAfterOpen => t!("admin-auctions-create-field-end-after-open"),
        }
    }
}

/// 只在操作员提交时解析本地分钟。用户输入必须在浏览器时区内有唯一的 UTC 对应。
pub(super) fn parse_local_schedule_time(value: &str) -> Result<Timestamp, ScheduleIssue> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new_0();
        resolve_local_minute(value, |utc_millis| {
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
    resolve_local_minute(value, |utc_millis| {
        let utc = Timestamp::from_timestamp_millis(utc_millis)?;
        Some((minute_fields(utc), u32::from(utc.to_utc_datetime().second().unsigned_abs())))
    })
}

fn resolve_local_minute(
    value: &str,
    mut local_fields: impl FnMut(i64) -> Option<([i32; 5], u32)>,
) -> Result<Timestamp, ScheduleIssue> {
    if value.is_empty() {
        return Err(ScheduleIssue::Required);
    }
    if value.len() != 16 || value.as_bytes()[10] != b'T' {
        return Err(ScheduleIssue::InvalidTime);
    }
    // 仅用这一时刻表示输入的年月日时分，实际 UTC 由下方反向核对确定。
    let wall = Timestamp::parse_from_rfc3339(&format!("{value}:00Z"))
        .map_err(|_error| ScheduleIssue::InvalidTime)?;
    let target = minute_fields(wall);
    let wall_millis = wall.timestamp_millis();
    let mut utc = None;
    // 时区偏移不足一天。遍历可能的分钟偏移并核对本地字段，避免假定回拨幅度为一小时。
    // 若偏移还包含秒，补正后再核对，禁止将输入静默移到缺口之后。
    for minutes in -1_440_i64..=1_440 {
        let probe = wall_millis + minutes * 60_000;
        let Some((fields, second)) = local_fields(probe) else {
            continue;
        };
        if fields != target {
            continue;
        }
        let candidate = probe - i64::from(second) * 1_000;
        if second != 0 && local_fields(candidate) != Some((target, 0)) {
            continue;
        }
        if utc.replace(candidate).is_some_and(|previous| previous != candidate) {
            return Err(ScheduleIssue::AmbiguousLocalTime);
        }
    }
    utc.and_then(Timestamp::from_timestamp_millis).ok_or(ScheduleIssue::NonexistentLocalTime)
}

fn minute_fields(timestamp: Timestamp) -> [i32; 5] {
    let fields = timestamp.to_utc_datetime();
    [
        i32::from(fields.year()),
        i32::from(fields.month()),
        i32::from(fields.day()),
        i32::from(fields.hour()),
        i32::from(fields.minute()),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScheduleMode {
    Create,
    Draft,
    Preview,
}

/// 创建和改排期都要求三项严格递增。已预览的场次只检查开拍仍在未来。
pub(super) fn validate_schedule(
    preview: Result<Timestamp, ScheduleIssue>,
    open: Result<Timestamp, ScheduleIssue>,
    end: Result<Timestamp, ScheduleIssue>,
    mode: ScheduleMode,
    now: Timestamp,
) -> Result<ScheduleTimes, (ScheduleField, ScheduleIssue)> {
    let preview = preview.map_err(|issue| (ScheduleField::Preview, issue))?;
    let open = open.map_err(|issue| (ScheduleField::Open, issue))?;
    let end = end.map_err(|issue| (ScheduleField::End, issue))?;
    if (mode == ScheduleMode::Create && preview < now)
        || (mode == ScheduleMode::Draft && preview <= now)
    {
        return Err((ScheduleField::Preview, ScheduleIssue::PreviewNotFuture));
    }
    if open <= preview {
        return Err((ScheduleField::Open, ScheduleIssue::OpenBeforePreview));
    }
    if mode == ScheduleMode::Preview && open <= now {
        return Err((ScheduleField::Open, ScheduleIssue::OpenNotFuture));
    }
    if end <= open {
        return Err((ScheduleField::End, ScheduleIssue::EndNotAfterOpen));
    }
    Ok(ScheduleTimes { preview, open, end })
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{
        ScheduleField, ScheduleIssue, ScheduleMode, minute_fields, resolve_local_minute,
        validate_schedule,
    };

    fn utc(value: &str) -> Timestamp {
        Timestamp::parse_from_rfc3339(value).unwrap()
    }

    fn local_minute(probe: i64, offset_minutes: i64) -> Option<([i32; 5], u32)> {
        let local = Timestamp::from_timestamp_millis(probe + offset_minutes * 60_000)?;
        Some((minute_fields(local), 0))
    }

    #[test]
    fn creation_requires_open_strictly_after_preview() {
        let now = utc("2099-04-01T11:00:00Z");
        let preview = Ok(utc("2099-04-01T12:00:00Z"));
        let open = Ok(utc("2099-04-01T13:00:00Z"));
        let end = Ok(utc("2099-04-01T14:00:00Z"));
        assert!(validate_schedule(preview, open, end, ScheduleMode::Create, now).is_ok());
        assert_eq!(
            validate_schedule(preview, preview, end, ScheduleMode::Create, now),
            Err((ScheduleField::Open, ScheduleIssue::OpenBeforePreview))
        );
    }

    #[test]
    fn spring_forward_accepts_a_valid_minute_and_rejects_a_skipped_one() {
        let transition = utc("2027-03-14T10:00:00Z").timestamp_millis();
        let format_local =
            |probe| local_minute(probe, if probe < transition { -480 } else { -420 });
        assert_eq!(
            resolve_local_minute("2027-03-14T03:30", format_local),
            Ok(utc("2027-03-14T10:30:00Z"))
        );
        assert_eq!(
            resolve_local_minute("2027-03-14T02:30", format_local),
            Err(ScheduleIssue::NonexistentLocalTime)
        );
        assert_eq!(
            resolve_local_minute("2027-02-29T12:00", format_local),
            Err(ScheduleIssue::InvalidTime)
        );
    }

    #[test]
    fn rejects_repeated_minutes_without_assuming_a_one_hour_shift() {
        let transition = utc("2027-11-07T09:00:00Z").timestamp_millis();
        for shift in [30, 60] {
            let format_local =
                |probe| local_minute(probe, if probe < transition { -420 } else { -420 - shift });
            assert_eq!(
                resolve_local_minute("2027-11-07T01:45", format_local),
                Err(ScheduleIssue::AmbiguousLocalTime)
            );
            assert_eq!(
                resolve_local_minute("2027-11-07T02:00", format_local),
                Ok(utc(if shift == 30 { "2027-11-07T09:30:00Z" } else { "2027-11-07T10:00:00Z" }))
            );
        }
    }
}
