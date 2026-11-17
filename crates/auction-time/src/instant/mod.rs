//! UTC 时刻：纳秒精度，不带时区。
//!
//! 文本形式是本类型的契约：序列化、`Display`、`Debug` 与各 RFC 3339 变体的输出逐字节固定，
//! 命令幂等摘要、证据摘要与已入库的 JSON 都依赖它，改动格式等于改动这些已持久化的事实。
//! 方法名沿用全仓既有调用点（`timestamp_micros`、`checked_add_signed` 等）。

mod calendar;
mod clock;
mod format;
mod http_date;
mod parse;
#[cfg(test)]
mod tests;
mod wire;

use std::ops::Add;
use std::ops::AddAssign;
use std::ops::Sub;
use std::ops::SubAssign;
use std::str::FromStr;

use jiff::civil;
use jiff::tz::Offset;

use crate::duration::Duration;

pub use format::SecondsFormat;
pub use parse::{ParseError, ParseErrorKind};

/// UTC 时刻。
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(jiff::Timestamp);

impl Timestamp {
    /// Unix 纪元：1970-01-01T00:00:00Z。
    pub const UNIX_EPOCH: Self = Self(jiff::Timestamp::UNIX_EPOCH);
    /// 可表示的最早时刻。
    pub const MIN_UTC: Self = Self(jiff::Timestamp::MIN);
    /// 可表示的最晚时刻。
    pub const MAX_UTC: Self = Self(jiff::Timestamp::MAX);

    /// 读取系统时钟；浏览器目标读取 `Date.now()`。
    pub fn now() -> Self {
        clock::system_now()
    }

    /// 由 Unix 秒与纳秒构造；纳秒须小于一秒，超出可表示范围时返回 `None`。
    pub fn from_timestamp(seconds: i64, nanoseconds: u32) -> Option<Self> {
        let nanoseconds = i32::try_from(nanoseconds).ok().filter(|value| *value < 1_000_000_000)?;
        jiff::Timestamp::new(seconds, nanoseconds).ok().map(Self)
    }

    /// 由 Unix 毫秒构造。
    pub fn from_timestamp_millis(milliseconds: i64) -> Option<Self> {
        jiff::Timestamp::from_millisecond(milliseconds).ok().map(Self)
    }

    /// 由 Unix 微秒构造。
    pub fn from_timestamp_micros(microseconds: i64) -> Option<Self> {
        jiff::Timestamp::from_microsecond(microseconds).ok().map(Self)
    }

    /// 由 Unix 纳秒构造。
    pub fn from_timestamp_nanos(nanoseconds: i64) -> Option<Self> {
        jiff::Timestamp::from_nanosecond(i128::from(nanoseconds)).ok().map(Self)
    }

    /// 由 UTC 的年月日时分秒构造；日期或时间无效时返回 `None`。
    pub fn from_ymd_hms(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Option<Self> {
        let datetime = civil::DateTime::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
            i8::try_from(hour).ok()?,
            i8::try_from(minute).ok()?,
            i8::try_from(second).ok()?,
            0,
        )
        .ok()?;
        Offset::UTC.to_timestamp(datetime).ok().map(Self)
    }

    /// Unix 秒，向下取整。
    pub fn timestamp(self) -> i64 {
        self.floor_parts().0
    }

    /// Unix 毫秒，向下取整。
    pub fn timestamp_millis(self) -> i64 {
        let (seconds, nanoseconds) = self.floor_parts();
        seconds.saturating_mul(1_000).saturating_add(i64::from(nanoseconds / 1_000_000))
    }

    /// Unix 微秒，向下取整。
    pub fn timestamp_micros(self) -> i64 {
        let (seconds, nanoseconds) = self.floor_parts();
        seconds.saturating_mul(1_000_000).saturating_add(i64::from(nanoseconds / 1_000))
    }

    /// Unix 纳秒；超出 `i64`（约 1677–2262 年之外）时返回 `None`。
    ///
    /// 全仓以它是否为 `Some` 作为“时刻在合理范围内”的校验口径。
    pub fn timestamp_nanos_opt(self) -> Option<i64> {
        i64::try_from(self.0.as_nanosecond()).ok()
    }

    /// 当前秒内的毫秒数，0 到 999。
    pub fn timestamp_subsec_millis(self) -> u32 {
        self.floor_parts().1 / 1_000_000
    }

    /// 当前秒内的微秒数，0 到 999999。
    pub fn timestamp_subsec_micros(self) -> u32 {
        self.floor_parts().1 / 1_000
    }

    /// 当前秒内的纳秒数，0 到 999999999。
    pub fn timestamp_subsec_nanos(self) -> u32 {
        self.floor_parts().1
    }

    /// 加上时长，超出可表示范围时返回 `None`。
    pub fn checked_add_signed(self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration.to_jiff()).ok().map(Self)
    }

    /// 减去时长，超出可表示范围时返回 `None`。
    pub fn checked_sub_signed(self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration.to_jiff()).ok().map(Self)
    }

    /// 本时刻减去 `earlier` 的有符号时长。
    pub fn signed_duration_since(self, earlier: Self) -> Duration {
        Duration::from(self.0.duration_since(earlier.0))
    }

    /// 严格按 RFC 3339 解析：日期与时间各字段定宽，分隔符为 `T`、`t` 或空格，偏移为 `Z` 或 `±hh:mm`。
    pub fn parse_from_rfc3339(text: &str) -> Result<Self, ParseError> {
        parse::strict(text)
    }

    /// RFC 3339 文本，UTC 偏移写作 `+00:00`，小数位按需取 0、3、6 或 9 位。
    pub fn to_rfc3339(self) -> String {
        self.to_rfc3339_opts(SecondsFormat::AutoSi, false)
    }

    /// RFC 3339 文本；`use_z` 为真时 UTC 偏移写作 `Z`，否则写作 `+00:00`。
    pub fn to_rfc3339_opts(self, seconds_format: SecondsFormat, use_z: bool) -> String {
        format::Rfc3339 { timestamp: self, seconds_format, use_z }.to_string()
    }

    /// UTC 下的日期与时间字段。
    pub fn to_utc_datetime(self) -> civil::DateTime {
        Offset::UTC.to_datetime(self.0)
    }

    /// 把日期与时间字段当作 UTC 得到时刻；超出可表示范围时返回 `None`。
    pub fn from_utc_datetime(datetime: civil::DateTime) -> Option<Self> {
        Offset::UTC.to_timestamp(datetime).ok().map(Self)
    }

    /// 底层的 jiff 时刻，供日历与时区运算使用。
    pub const fn to_jiff(self) -> jiff::Timestamp {
        self.0
    }

    /// 向下取整的 Unix 秒与秒内的非负纳秒。
    fn floor_parts(self) -> (i64, u32) {
        let (seconds, nanoseconds) = (self.0.as_second(), self.0.subsec_nanosecond());
        if nanoseconds < 0 {
            (seconds - 1, (nanoseconds + 1_000_000_000).unsigned_abs())
        } else {
            (seconds, nanoseconds.unsigned_abs())
        }
    }
}

impl From<std::time::SystemTime> for Timestamp {
    /// 超出可表示范围的系统时间饱和到最早或最晚时刻。
    fn from(system_time: std::time::SystemTime) -> Self {
        match jiff::Timestamp::try_from(system_time) {
            Ok(timestamp) => Self(timestamp),
            Err(_error) if system_time < std::time::SystemTime::UNIX_EPOCH => Self::MIN_UTC,
            Err(_error) => Self::MAX_UTC,
        }
    }
}

impl From<jiff::Timestamp> for Timestamp {
    fn from(timestamp: jiff::Timestamp) -> Self {
        Self(timestamp)
    }
}

impl From<Timestamp> for jiff::Timestamp {
    fn from(timestamp: Timestamp) -> Self {
        timestamp.0
    }
}

impl FromStr for Timestamp {
    type Err = ParseError;

    /// 宽松的 RFC 3339：字段可不补零、各部分之间可有空白，偏移还接受 `UTC`。
    fn from_str(text: &str) -> Result<Self, ParseError> {
        parse::relaxed(text)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        Self(self.0 + duration.to_jiff())
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    fn sub(self, duration: Duration) -> Self {
        Self(self.0 - duration.to_jiff())
    }
}

impl Sub for Timestamp {
    type Output = Duration;

    fn sub(self, earlier: Self) -> Duration {
        self.signed_duration_since(earlier)
    }
}

impl AddAssign<Duration> for Timestamp {
    fn add_assign(&mut self, duration: Duration) {
        self.0 += duration.to_jiff();
    }
}

impl SubAssign<Duration> for Timestamp {
    fn sub_assign(&mut self, duration: Duration) {
        self.0 -= duration.to_jiff();
    }
}
