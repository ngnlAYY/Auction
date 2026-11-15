//! Admin Users owner 共享字段校验。

use chrono::{DateTime, Utc};

pub(super) fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}
