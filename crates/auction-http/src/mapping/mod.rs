//! 多个 handler 模块共用的 wire 投影：Admin/Public 时间戳格式，以及 Public 与 Admin 共用 v1
//! 定长帧的 keyset 游标编解码。帧格式与类型字节需要统一登记，因此即使某种游标只有一个调用方，
//! 编解码也集中在 `cursor`；单一 owner 的 DTO 映射留在各自的 handler 模块，不放进这里。

mod cursor;
#[cfg(test)]
pub(crate) mod fixtures;
mod listing_v2;
mod timestamp;

pub(crate) use cursor::{
    decode_admin_proxy_event_cursor, decode_public_bid_log_cursor,
    decode_public_compensation_cursor, decode_public_cursor, decode_public_price_history_cursor,
    encode_admin_proxy_event_cursor, encode_public_bid_log_cursor,
    encode_public_compensation_cursor, encode_public_cursor, encode_public_price_history_cursor,
};
pub(crate) use listing_v2::owned_v2;
pub(crate) use timestamp::{public_timestamp, timestamp};
