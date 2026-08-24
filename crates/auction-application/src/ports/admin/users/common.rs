//! Admin Users owner 共享字段校验。

use chrono::{DateTime, Utc};

pub(super) fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| value.is_empty() || valid_text(value, maximum))
}

pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
