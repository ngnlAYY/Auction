//! Admin Users owner 共享字段校验。

pub(super) use auction_domain::valid_text;
use chrono::{DateTime, Utc};

pub(super) fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
