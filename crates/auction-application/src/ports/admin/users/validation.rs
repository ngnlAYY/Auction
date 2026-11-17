//! Admin Users owner 共享字段校验。

use auction_time::Timestamp;

pub(super) fn valid_time(value: Timestamp) -> bool {
    value.timestamp_nanos_opt().is_some()
}
