//! 日历日期与钟点的宽松文本解析。
//!
//! 接受范围沿用全仓既有输入：数字字段之前可有空白、可不补零，年份可带符号，末尾不得有多余
//! 内容。部署配置、运维回执与外部页面一直按这一范围书写，收紧会让已经接受的文本失效；
//! 需要规范写法的调用方在解析后再把结果格式化回去比较。

#[cfg(test)]
mod tests;

use jiff::civil;

/// 解析 `时:分`，例如 `18:00`、`9:05`。
pub fn parse_clock(text: &str) -> Option<civil::Time> {
    let mut scanner = Scanner(text);
    let (hour, minute) = scanner.clock()?;
    scanner.end()?;
    civil::Time::new(hour, minute, 0, 0).ok()
}

/// 解析 `年-月-日`，例如 `2026-11-05`。
pub fn parse_date(text: &str) -> Option<civil::Date> {
    let mut scanner = Scanner(text);
    let date = scanner.date()?;
    scanner.end()?;
    Some(date)
}

/// 解析 `年-月-日 时:分`；日期与钟点之间的空白数量不限，也可以没有。
pub fn parse_minute_datetime(text: &str) -> Option<civil::DateTime> {
    let mut scanner = Scanner(text);
    let date = scanner.date()?;
    scanner.0 = scanner.0.trim_start();
    let (hour, minute) = scanner.clock()?;
    scanner.end()?;
    Some(date.to_datetime(civil::Time::new(hour, minute, 0, 0).ok()?))
}

/// 从左到右消费文本的游标；任何一步失败整个解析即失败。
struct Scanner<'a>(&'a str);

impl Scanner<'_> {
    fn date(&mut self) -> Option<civil::Date> {
        let year = self.year()?;
        self.literal('-')?;
        let month = self.number(2)?;
        self.literal('-')?;
        let day = self.number(2)?;
        civil::Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
    }

    fn clock(&mut self) -> Option<(i8, i8)> {
        let hour = self.number(2)?;
        self.literal(':')?;
        let minute = self.number(2)?;
        Some((i8::try_from(hour).ok()?, i8::try_from(minute).ok()?))
    }

    /// 年份：带 `+` 或 `-` 时位数不限，符号与数字之间不得有空白；不带符号时至多四位。
    fn year(&mut self) -> Option<i64> {
        self.0 = self.0.trim_start();
        if let Some(rest) = self.0.strip_prefix('-') {
            self.0 = rest;
            return self.digits(usize::MAX)?.checked_neg();
        }
        if let Some(rest) = self.0.strip_prefix('+') {
            self.0 = rest;
            return self.digits(usize::MAX);
        }
        self.digits(4)
    }

    /// 数字字段：先跳过空白，再取 1 到 `max` 位 ASCII 数字。
    fn number(&mut self, max: usize) -> Option<i64> {
        self.0 = self.0.trim_start();
        self.digits(max)
    }

    fn digits(&mut self, max: usize) -> Option<i64> {
        let count = self.0.bytes().take(max).take_while(u8::is_ascii_digit).count();
        if count == 0 {
            return None;
        }
        let (digits, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        digits.parse().ok()
    }

    fn literal(&mut self, expected: char) -> Option<()> {
        self.0 = self.0.strip_prefix(expected)?;
        Some(())
    }

    fn end(&self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}
