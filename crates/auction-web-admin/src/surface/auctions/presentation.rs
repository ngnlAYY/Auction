//! Admin 拍卖页面共享的状态、时间和文案映射。

use auction_time::Timestamp;
use auction_web_shared::t;

use crate::api::{LocalTimeError, parse_local_datetime};

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
    cfg_select! {
        target_arch = "wasm32" => js_sys::Date::new_0().get_time() as i64,
        _ => auction_time::Timestamp::now().timestamp_millis(),
    }
}

pub(super) fn scheduled_time_millis(value: &str) -> Option<i64> {
    cfg_select! {
        target_arch = "wasm32" => {
            let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
            let milliseconds = date.get_time();
            milliseconds.is_finite().then_some(milliseconds as i64)
        }
        _ => auction_time::Timestamp::parse_from_rfc3339(value)
            .ok()
            .map(auction_time::Timestamp::timestamp_millis),
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
    if value.is_empty() {
        return Err(ScheduleIssue::Required);
    }
    if value.len() != 16 || value.as_bytes()[10] != b'T' {
        return Err(ScheduleIssue::InvalidTime);
    }
    parse_local_datetime(value).map_err(|error| match error {
        LocalTimeError::Invalid => ScheduleIssue::InvalidTime,
        LocalTimeError::Nonexistent => ScheduleIssue::NonexistentLocalTime,
        LocalTimeError::Ambiguous => ScheduleIssue::AmbiguousLocalTime,
    })
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
        ScheduleField, ScheduleIssue, ScheduleMode, parse_local_schedule_time, validate_schedule,
    };

    fn utc(value: &str) -> Timestamp {
        Timestamp::parse_from_rfc3339(value).unwrap()
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
    fn schedule_still_requires_local_minute_input() {
        assert_eq!(parse_local_schedule_time(""), Err(ScheduleIssue::Required));
        assert_eq!(parse_local_schedule_time("2027-03-14T03:30"), Ok(utc("2027-03-14T03:30:00Z")));
        for input in ["2027-03-14T03:30:00", "2027-03-14 03:30", "2027-02-29T12:00"] {
            assert_eq!(parse_local_schedule_time(input), Err(ScheduleIssue::InvalidTime));
        }
    }
}
