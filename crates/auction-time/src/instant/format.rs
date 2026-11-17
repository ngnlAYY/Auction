//! 时刻的文本输出：RFC 3339 各变体、`Display` 与 `Debug`。
//!
//! 年份在 0–9999 之内写四位，之外带符号至少五位；秒的小数位由 [`SecondsFormat`] 决定。

use std::fmt;

use jiff::civil;

use super::Timestamp;

/// RFC 3339 文本里秒以下的位数。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecondsFormat {
    /// 不写小数。
    Secs,
    /// 固定三位。
    Millis,
    /// 固定六位。
    Micros,
    /// 固定九位。
    Nanos,
    /// 按值取 0、3、6 或 9 位中能精确表示的最短一种。
    AutoSi,
}

/// 一次 RFC 3339 输出的参数；`Display` 即输出。
pub(super) struct Rfc3339 {
    pub(super) timestamp: Timestamp,
    pub(super) seconds_format: SecondsFormat,
    pub(super) use_z: bool,
}

impl fmt::Display for Rfc3339 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let datetime = self.timestamp.to_utc_datetime();
        write_date(formatter, datetime.date())?;
        formatter.write_str("T")?;
        write_time(formatter, datetime.time(), self.seconds_format)?;
        formatter.write_str(if self.use_z { "Z" } else { "+00:00" })
    }
}

impl fmt::Display for Timestamp {
    /// `2026-11-17 10:00:00.123 UTC`：日期与时间之间是空格，小数位按 [`SecondsFormat::AutoSi`]。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let datetime = self.to_utc_datetime();
        write_date(formatter, datetime.date())?;
        formatter.write_str(" ")?;
        write_time(formatter, datetime.time(), SecondsFormat::AutoSi)?;
        formatter.write_str(" UTC")
    }
}

impl fmt::Debug for Timestamp {
    /// `2026-11-17T10:00:00.123Z`，与序列化文本相同。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(
            &Rfc3339 { timestamp: *self, seconds_format: SecondsFormat::AutoSi, use_z: true },
            formatter,
        )
    }
}

fn write_date(formatter: &mut fmt::Formatter<'_>, date: civil::Date) -> fmt::Result {
    let year = i32::from(date.year());
    if (0..=9999).contains(&year) {
        write!(formatter, "{year:04}")?;
    } else {
        // ISO 8601 要求范围之外的年份显式带符号。
        write!(formatter, "{year:+05}")?;
    }
    write!(formatter, "-{:02}-{:02}", date.month(), date.day())
}

fn write_time(
    formatter: &mut fmt::Formatter<'_>,
    time: civil::Time,
    seconds_format: SecondsFormat,
) -> fmt::Result {
    write!(formatter, "{:02}:{:02}:{:02}", time.hour(), time.minute(), time.second())?;
    let nanoseconds = time.subsec_nanosecond();
    match seconds_format {
        SecondsFormat::Secs => Ok(()),
        SecondsFormat::Millis => write!(formatter, ".{:03}", nanoseconds / 1_000_000),
        SecondsFormat::Micros => write!(formatter, ".{:06}", nanoseconds / 1_000),
        SecondsFormat::Nanos => write!(formatter, ".{nanoseconds:09}"),
        SecondsFormat::AutoSi if nanoseconds == 0 => Ok(()),
        SecondsFormat::AutoSi if nanoseconds % 1_000_000 == 0 => {
            write!(formatter, ".{:03}", nanoseconds / 1_000_000)
        }
        SecondsFormat::AutoSi if nanoseconds % 1_000 == 0 => {
            write!(formatter, ".{:06}", nanoseconds / 1_000)
        }
        SecondsFormat::AutoSi => write!(formatter, ".{nanoseconds:09}"),
    }
}
