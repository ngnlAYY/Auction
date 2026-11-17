//! 全仓共用的 UTC 时刻与有符号时长；文本格式、数据库编码与本地墙上时间换算的唯一 owner。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod civil_text;
mod duration;
mod instant;
mod local_time;
#[cfg(feature = "postgres")]
mod postgres;

pub use civil_text::{parse_clock, parse_date, parse_minute_datetime};
pub use duration::{Duration, OutOfRangeError, TimeDelta};
pub use instant::{ParseError, ParseErrorKind, SecondsFormat, Timestamp};
pub use local_time::LocalTime;
