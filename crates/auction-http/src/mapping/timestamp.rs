//! Admin 与 Public 两套既有时间戳 wire 格式。

use auction_application::auction::format_public_time_wire;
use auction_time::{SecondsFormat, Timestamp};

/// Admin 契约的 RFC 3339 微秒 UTC 时间戳。
pub(crate) fn timestamp(value: Timestamp) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Public 按已发布的 Go `time.Time` `RFC3339Nano` 格式输出；Admin 继续使用上方既有微秒契约。
pub(crate) fn public_timestamp(value: Timestamp) -> String {
    format_public_time_wire(value)
}
