//! 排期来源本地钟点的 JSON 表示：`YYYY-MM-DDTHH:MM:SS`，非整秒时追加 3、6 或 9 位小数。
//!
//! 冻结来源随轮次快照与命令账本持久化；写出的文本逐字节保持既有格式，已入库的快照与按其计算的
//! 摘要不因时间库更换而改变。读取只接受本模块会写出的规范文本：解析后重新写出必须与原文一致，
//! 否则失败关闭。闰秒（秒为 60）无法表示，不能读成 59 秒继续使用。

use std::fmt;

use jiff::civil;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;

pub(super) fn serialize<S: Serializer>(
    value: &civil::DateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&LocalText(*value))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<civil::DateTime, D::Error> {
    let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
    parse(&text)
        .ok_or_else(|| D::Error::custom("排期本地钟点不是规范的 YYYY-MM-DDTHH:MM:SS[.小数]"))
}

/// 按字段读取后重新写出，与原文逐字符一致才接受；字段取值无效（含秒为 60）时返回 `None`。
fn parse(text: &str) -> Option<civil::DateTime> {
    let (date, time) = text.split_once('T')?;
    // 年份可带符号（公元 0–9999 年之外的写法），其余日期字段以 `-` 分隔。
    let (negative, unsigned) = match date.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, date.trim_prefix('+')),
    };
    let mut date_fields = unsigned.split('-');
    let year = digits(date_fields.next()?)?;
    let month = digits(date_fields.next()?)?;
    let day = digits(date_fields.next()?)?;
    if date_fields.next().is_some() {
        return None;
    }
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };
    let mut clock_fields = clock.split(':');
    let hour = digits(clock_fields.next()?)?;
    let minute = digits(clock_fields.next()?)?;
    let second = digits(clock_fields.next()?)?;
    if clock_fields.next().is_some() {
        return None;
    }
    let nanosecond = match fraction {
        Some(fraction) if fraction.len() <= 9 => {
            let scale = 10_i64.checked_pow(u32::try_from(9 - fraction.len()).ok()?)?;
            digits(fraction)?.checked_mul(scale)?
        }
        Some(_) => return None,
        None => 0,
    };
    let year = if negative { year.checked_neg()? } else { year };
    let value = civil::DateTime::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
        i32::try_from(nanosecond).ok()?,
    )
    .ok()?;
    (LocalText(value).to_string() == text).then_some(value)
}

/// 非空的纯 ASCII 数字串；位数是否规范由调用方的往返比较判定。
fn digits(text: &str) -> Option<i64> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())?
}

struct LocalText(civil::DateTime);

impl fmt::Display for LocalText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        // 公元 0–9999 年之外的年份带符号、至少四位，与既有文本一致。
        if (0..=9999).contains(&value.year()) {
            write!(formatter, "{:04}", value.year())?;
        } else {
            write!(formatter, "{:+05}", value.year())?;
        }
        write!(
            formatter,
            "-{:02}-{:02}T{:02}:{:02}:{:02}",
            value.month(),
            value.day(),
            value.hour(),
            value.minute(),
            value.second()
        )?;
        let nanos = value.subsec_nanosecond();
        if nanos == 0 {
            Ok(())
        } else if nanos % 1_000_000 == 0 {
            write!(formatter, ".{:03}", nanos / 1_000_000)
        } else if nanos % 1_000 == 0 {
            write!(formatter, ".{:06}", nanos / 1_000)
        } else {
            write!(formatter, ".{nanos:09}")
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil;

    use super::LocalText;
    use super::parse;

    /// 写出的文本固定，且都能原样读回（含公元 0–9999 年之外带符号的年份）。
    #[test]
    fn written_text_is_fixed_and_reads_back() {
        for (value, text) in [
            (civil::datetime(2026, 11, 1, 9, 0, 0, 0), "2026-11-01T09:00:00"),
            (civil::datetime(2026, 11, 1, 9, 0, 0, 250_000_000), "2026-11-01T09:00:00.250"),
            (civil::datetime(2026, 11, 1, 9, 0, 0, 123_000), "2026-11-01T09:00:00.000123"),
            (civil::datetime(2026, 11, 1, 9, 0, 0, 1), "2026-11-01T09:00:00.000000001"),
            (civil::datetime(0, 1, 1, 0, 0, 0, 0), "0000-01-01T00:00:00"),
            (civil::datetime(9999, 12, 31, 23, 59, 59, 0), "9999-12-31T23:59:59"),
            (civil::datetime(-1, 1, 1, 0, 0, 0, 0), "-0001-01-01T00:00:00"),
        ] {
            assert_eq!(LocalText(value).to_string(), text);
            assert_eq!(parse(text), Some(value), "{text}");
        }
    }

    /// 只接受规范文本：闰秒不能读成 59 秒，其余非规范写法一律拒绝而不是悄悄归一。
    #[test]
    fn leap_seconds_and_non_canonical_text_are_rejected() {
        for text in [
            "2026-12-31T23:59:60",
            "2026-12-31T23:59:60.500",
            "2026-11-01T09:00",
            "2026-11-01 09:00:00",
            "2026-11-01t09:00:00",
            "2026-11-01T09:00:00Z",
            "2026-11-01T09:00:00+08:00",
            "2026-11-01T09:00:00.5",
            "2026-11-01T09:00:00.000",
            "2026-11-01T09:00:00.1234567890",
            "2026-1-01T09:00:00",
            "2026-11-01T9:00:00",
            "+2026-11-01T09:00:00",
            "-000001-01-01T00:00:00",
            "20261101T090000",
            "2026-02-30T09:00:00",
            "2026-11-01T24:00:00",
            " 2026-11-01T09:00:00",
            "",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }
}
