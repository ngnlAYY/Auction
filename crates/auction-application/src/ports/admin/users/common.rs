//! Admin Users owner 共享字段校验。

pub(super) use auction_domain::valid_text;
use chrono::{DateTime, Utc};

pub(super) fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| value.is_empty() || valid_text(value, maximum))
}

pub(super) fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
