//! RFC 3339 文本的解析：严格形式与宽松形式。
//!
//! 两种形式的接受范围与历史实现逐项对齐，校验顺序也保持一致，因此同一输入得到同一类错误：
//!
//! - 严格形式：`YYYY-MM-DD` + `T`/`t`/空格 + `hh:mm:ss[.小数]` + `Z`/`z` 或 `±hh:mm`，各字段定宽。
//! - 宽松形式：字段可不补零、各部分之间可有空白、年份可带符号，偏移还接受 `UTC` 与省略冒号。
//!
//! 小数秒取前九位，多余的数字丢弃。闰秒（秒为 60）无法用 UTC 时刻表示，按超出范围拒绝。

use std::fmt;

use jiff::civil;
use jiff::tz::Offset;

use super::Timestamp;

/// 解析失败的类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// 字段语法正确，但数值超出允许范围，或日期不存在。
    OutOfRange,
    /// 输入含有不该出现的字符。
    Invalid,
    /// 输入提前结束。
    TooShort,
    /// 完整的时刻之后还有多余内容。
    TooLong,
}

/// RFC 3339 文本解析失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParseError(ParseErrorKind);

impl ParseError {
    /// 失败类别。
    pub const fn kind(&self) -> ParseErrorKind {
        self.0
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            ParseErrorKind::OutOfRange => "input is out of range",
            ParseErrorKind::Invalid => "input contains invalid characters",
            ParseErrorKind::TooShort => "premature end of input",
            ParseErrorKind::TooLong => "trailing input",
        })
    }
}

impl std::error::Error for ParseError {}

const OUT_OF_RANGE: ParseError = ParseError(ParseErrorKind::OutOfRange);
const INVALID: ParseError = ParseError(ParseErrorKind::Invalid);
const TOO_SHORT: ParseError = ParseError(ParseErrorKind::TooShort);
const TOO_LONG: ParseError = ParseError(ParseErrorKind::TooLong);

type ParseResult<T> = Result<T, ParseError>;

/// 偏移里小时与分钟之间的分隔规则。
#[derive(Clone, Copy)]
enum OffsetSeparator {
    /// 必须恰好是一个冒号。
    Colon,
    /// 任意个冒号或空白，可以没有。
    ColonOrSpace,
}

/// 严格形式。
pub(super) fn strict(text: &str) -> ParseResult<Timestamp> {
    let Some(fixed) = text.as_bytes().first_chunk::<19>() else {
        return Err(TOO_SHORT);
    };
    let year = digit(fixed, 0)? * 1_000
        + digit(fixed, 1)? * 100
        + digit(fixed, 2)? * 10
        + digit(fixed, 3)?;
    expect_byte(fixed, 4, b'-')?;
    let month = two_digits(fixed, 5)?;
    expect_byte(fixed, 7, b'-')?;
    let day = two_digits(fixed, 8)?;
    let date = civil_date(year, month, day)?;
    if !matches!(fixed[10], b't' | b'T' | b' ') {
        return Err(INVALID);
    }
    let hour = two_digits(fixed, 11)?;
    expect_byte(fixed, 13, b':')?;
    let minute = two_digits(fixed, 14)?;
    expect_byte(fixed, 16, b':')?;
    let second = two_digits(fixed, 17)?;
    // 前 19 个字节已确认全是 ASCII，在这里切分不会落在字符中间。
    let rest = text.get(19..).ok_or(INVALID)?;
    let (rest, nanosecond) = match rest.strip_prefix('.') {
        Some(fraction) => nanosecond(fraction)?,
        None => (rest, 0),
    };
    let (time, leap_second) = civil_time(hour, minute, second, nanosecond)?;
    let (rest, offset_seconds) = timezone_offset(rest, OffsetSeparator::Colon)?;
    if !rest.is_empty() {
        return Err(TOO_LONG);
    }
    to_timestamp(date, time, leap_second, offset_seconds)
}

/// 宽松形式。
pub(super) fn relaxed(text: &str) -> ParseResult<Timestamp> {
    let (rest, year) = signed_year(text)?;
    let rest = literal(rest.trim_start(), '-')?;
    let (rest, month) = field(rest, 1..=12)?;
    let rest = literal(rest.trim_start(), '-')?;
    let (rest, day) = field(rest, 1..=31)?;
    let rest = match rest.as_bytes().first() {
        Some(b't' | b'T' | b' ') => rest.get(1..).ok_or(INVALID)?,
        Some(_) => return Err(INVALID),
        None => return Err(TOO_SHORT),
    };
    let (rest, hour) = field(rest, 0..=23)?;
    let rest = literal(rest.trim_start(), ':')?;
    let (rest, minute) = field(rest, 0..=59)?;
    let rest = literal(rest.trim_start(), ':')?;
    let (rest, second) = field(rest, 0..=60)?;
    let (rest, nanosecond) = match rest.strip_prefix('.') {
        Some(fraction) => nanosecond(fraction)?,
        None => (rest, 0),
    };
    let rest = rest.trim_start();
    let (rest, offset_seconds) = match rest.as_bytes().first_chunk::<3>() {
        Some(prefix) if prefix.eq_ignore_ascii_case(b"UTC") => (rest.get(3..).ok_or(INVALID)?, 0),
        _ => timezone_offset(rest, OffsetSeparator::ColonOrSpace)?,
    };
    if !rest.trim_start().is_empty() {
        return Err(TOO_LONG);
    }
    let date = civil_date(year, month, day)?;
    let (time, leap_second) = civil_time(hour, minute, second, nanosecond)?;
    to_timestamp(date, time, leap_second, offset_seconds)
}

/// 年份：无符号时一到四位；带 `+` 或 `-` 时位数不限。
fn signed_year(text: &str) -> ParseResult<(&str, i64)> {
    let text = text.trim_start();
    let (rest, year) = if let Some(digits) = text.strip_prefix('-') {
        let (rest, magnitude) = number(digits, 1, usize::MAX)?;
        (rest, 0_i64.checked_sub(magnitude).ok_or(OUT_OF_RANGE)?)
    } else if let Some(digits) = text.strip_prefix('+') {
        number(digits, 1, usize::MAX)?
    } else {
        number(text, 1, 4)?
    };
    if i32::try_from(year).is_err() {
        return Err(OUT_OF_RANGE);
    }
    Ok((rest, year))
}

/// 一到两位的无符号字段，前面可有空白；数值须落在 `range` 内。
fn field(text: &str, range: std::ops::RangeInclusive<i64>) -> ParseResult<(&str, i64)> {
    let (rest, value) = number(text.trim_start(), 1, 2)?;
    if !range.contains(&value) {
        return Err(OUT_OF_RANGE);
    }
    Ok((rest, value))
}

fn literal(text: &str, expected: char) -> ParseResult<&str> {
    if text.is_empty() {
        return Err(TOO_SHORT);
    }
    text.strip_prefix(expected).ok_or(INVALID)
}

fn digit(fixed: &[u8; 19], index: usize) -> ParseResult<i64> {
    match fixed[index] {
        byte @ b'0'..=b'9' => Ok(i64::from(byte - b'0')),
        _ => Err(INVALID),
    }
}

fn two_digits(fixed: &[u8; 19], index: usize) -> ParseResult<i64> {
    Ok(digit(fixed, index)? * 10 + digit(fixed, index + 1)?)
}

fn expect_byte(fixed: &[u8; 19], index: usize, expected: u8) -> ParseResult<()> {
    (fixed[index] == expected).ok_or(INVALID)
}

/// 读取 `min` 到 `max` 位十进制数字，遇到第一个非数字停止。
fn number(text: &str, min: usize, max: usize) -> ParseResult<(&str, i64)> {
    let bytes = text.as_bytes();
    if bytes.len() < min {
        return Err(TOO_SHORT);
    }
    let mut value = 0_i64;
    let mut consumed = 0_usize;
    for byte in bytes.iter().copied().take(max) {
        if !byte.is_ascii_digit() {
            if consumed < min {
                return Err(INVALID);
            }
            break;
        }
        value = value
            .checked_mul(10)
            .and_then(|scaled| scaled.checked_add(i64::from(byte - b'0')))
            .ok_or(OUT_OF_RANGE)?;
        consumed += 1;
    }
    // 已消费的字节全是 ASCII 数字，切分点一定在字符边界上。
    Ok((text.get(consumed..).ok_or(INVALID)?, value))
}

/// 小数秒：至少一位数字，取前九位换算成纳秒，其余数字丢弃。
fn nanosecond(text: &str) -> ParseResult<(&str, u32)> {
    const SCALE: [u32; 10] =
        [0, 100_000_000, 10_000_000, 1_000_000, 100_000, 10_000, 1_000, 100, 10, 1];
    let (rest, value) = number(text, 1, 9)?;
    let consumed = text.len() - rest.len();
    let scale = SCALE.get(consumed).copied().ok_or(OUT_OF_RANGE)?;
    let nanoseconds = u32::try_from(value).ok().and_then(|value| value.checked_mul(scale));
    let rest = rest.trim_start_matches(|character: char| character.is_ascii_digit());
    Ok((rest, nanoseconds.ok_or(OUT_OF_RANGE)?))
}

/// UTC 偏移，返回秒数：`Z`/`z`，或符号（`+`、`-`、U+2212）加两位小时与两位分钟。
fn timezone_offset(text: &str, separator: OffsetSeparator) -> ParseResult<(&str, i32)> {
    if let Some(rest) = text.strip_prefix(['Z', 'z']) {
        return Ok((rest, 0));
    }
    let mut characters = text.chars();
    let negative = match characters.next() {
        Some('+') => false,
        Some('-' | '\u{2212}') => true,
        Some(_) => return Err(INVALID),
        None => return Err(TOO_SHORT),
    };
    let rest = characters.as_str();
    let hours = match rest.as_bytes() {
        [tens @ b'0'..=b'9', ones @ b'0'..=b'9', ..] => {
            i32::from((tens - b'0') * 10 + (ones - b'0'))
        }
        [_, _, ..] => return Err(INVALID),
        _ => return Err(TOO_SHORT),
    };
    let rest = rest.get(2..).ok_or(INVALID)?;
    let rest = match separator {
        OffsetSeparator::Colon => match rest.as_bytes().first() {
            Some(b':') => rest.get(1..).ok_or(INVALID)?,
            Some(_) => return Err(INVALID),
            None => return Err(TOO_SHORT),
        },
        OffsetSeparator::ColonOrSpace => {
            rest.trim_start_matches(|character: char| character == ':' || character.is_whitespace())
        }
    };
    let minutes = match rest.as_bytes() {
        [tens @ b'0'..=b'5', ones @ b'0'..=b'9', ..] => {
            i32::from((tens - b'0') * 10 + (ones - b'0'))
        }
        [b'6'..=b'9', b'0'..=b'9', ..] => return Err(OUT_OF_RANGE),
        [_, _, ..] => return Err(INVALID),
        _ => return Err(TOO_SHORT),
    };
    // 两位分钟不一定是 ASCII：非法字节已在上面按 INVALID 返回，这里的两个字节都是数字。
    let rest = rest.get(2..).ok_or(INVALID)?;
    let seconds = hours * 3_600 + minutes * 60;
    Ok((rest, if negative { -seconds } else { seconds }))
}

fn civil_date(year: i64, month: i64, day: i64) -> ParseResult<civil::Date> {
    let (Ok(year), Ok(month), Ok(day)) =
        (i16::try_from(year), i8::try_from(month), i8::try_from(day))
    else {
        return Err(OUT_OF_RANGE);
    };
    civil::Date::new(year, month, day).ok().ok_or(OUT_OF_RANGE)
}

/// 校验时间字段。秒为 60 的闰秒在语法上合法，这里先按 59 通过并用第二个返回值标出，
/// 留到全部语法检查之后再拒绝，使同时带有其他错误的输入报出与历史一致的错误类别。
fn civil_time(
    hour: i64,
    minute: i64,
    second: i64,
    nanosecond: u32,
) -> ParseResult<(civil::Time, bool)> {
    let leap_second = second == 60;
    let second = if leap_second { 59 } else { second };
    let (Ok(hour), Ok(minute), Ok(second), Ok(nanosecond)) =
        (i8::try_from(hour), i8::try_from(minute), i8::try_from(second), i32::try_from(nanosecond))
    else {
        return Err(OUT_OF_RANGE);
    };
    let time = civil::Time::new(hour, minute, second, nanosecond).ok().ok_or(OUT_OF_RANGE)?;
    Ok((time, leap_second))
}

fn to_timestamp(
    date: civil::Date,
    time: civil::Time,
    leap_second: bool,
    offset_seconds: i32,
) -> ParseResult<Timestamp> {
    // 闰秒无法用 UTC 时刻表示。
    if leap_second {
        return Err(OUT_OF_RANGE);
    }
    // 偏移的绝对值须小于一天。
    if offset_seconds.unsigned_abs() >= 86_400 {
        return Err(OUT_OF_RANGE);
    }
    let offset = Offset::from_seconds(offset_seconds).ok().ok_or(OUT_OF_RANGE)?;
    let timestamp = offset.to_timestamp(date.to_datetime(time)).ok().ok_or(OUT_OF_RANGE)?;
    Ok(Timestamp::from(timestamp))
}
