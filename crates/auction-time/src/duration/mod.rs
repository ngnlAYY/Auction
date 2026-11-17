//! 有符号时长：精度为纳秒，可为负。
//!
//! 构造与读取方法沿用全仓既有调用点的名字（`seconds`、`num_seconds` 等）；截断方向一律朝零，
//! 与历史行为一致。`Display` 输出只含秒的 ISO 8601 时长，`Debug` 输出规范化后的秒与纳秒。

#[cfg(test)]
mod tests;
mod wire;

use std::fmt;
use std::ops::Add;
use std::ops::AddAssign;
use std::ops::Div;
use std::ops::Mul;
use std::ops::Neg;
use std::ops::Sub;
use std::ops::SubAssign;

use jiff::SignedDuration;

/// 有符号时长。
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(SignedDuration);

/// 历史别名：部分模块同时使用标准库的 `Duration`，以本名区分。
pub type TimeDelta = Duration;

/// 与标准库时长互转时超出可表示范围（例如负值无法转成 `std::time::Duration`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfRangeError;

impl fmt::Display for OutOfRangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("source duration value is out of range for the target type")
    }
}

impl std::error::Error for OutOfRangeError {}

impl Duration {
    /// 零时长。
    pub const ZERO: Self = Self(SignedDuration::ZERO);
    /// 历史取值范围的上限：`i64::MAX` 毫秒。
    pub const MAX: Self = Self(SignedDuration::new(i64::MAX / 1_000, 807_000_000));

    /// 由整秒构造。
    pub const fn seconds(seconds: i64) -> Self {
        Self(SignedDuration::from_secs(seconds))
    }

    /// 由整分钟构造；超出范围时饱和。
    pub const fn minutes(minutes: i64) -> Self {
        Self::seconds(minutes.saturating_mul(60))
    }

    /// 由整小时构造；超出范围时饱和。
    pub const fn hours(hours: i64) -> Self {
        Self::seconds(hours.saturating_mul(3_600))
    }

    /// 由整天构造，一天固定为 86400 秒；超出范围时饱和。
    pub const fn days(days: i64) -> Self {
        Self::seconds(days.saturating_mul(86_400))
    }

    /// 由整周构造，一周固定为 7 天；超出范围时饱和。
    pub const fn weeks(weeks: i64) -> Self {
        Self::seconds(weeks.saturating_mul(604_800))
    }

    /// 由毫秒构造。
    pub const fn milliseconds(milliseconds: i64) -> Self {
        Self(SignedDuration::from_millis(milliseconds))
    }

    /// 由微秒构造。
    pub const fn microseconds(microseconds: i64) -> Self {
        Self(SignedDuration::from_micros(microseconds))
    }

    /// 由纳秒构造。
    pub const fn nanoseconds(nanoseconds: i64) -> Self {
        Self(SignedDuration::from_nanos(nanoseconds))
    }

    /// 零时长。
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// 由整秒构造；毫秒数超出 `i64` 时返回 `None`。
    pub fn try_seconds(seconds: i64) -> Option<Self> {
        Self::from_floor_parts(seconds, 0)
    }

    /// 由整分钟构造；超出范围时返回 `None`。
    pub fn try_minutes(minutes: i64) -> Option<Self> {
        Self::try_seconds(minutes.checked_mul(60)?)
    }

    /// 由整小时构造；超出范围时返回 `None`。
    pub fn try_hours(hours: i64) -> Option<Self> {
        Self::try_seconds(hours.checked_mul(3_600)?)
    }

    /// 由整天构造；超出范围时返回 `None`。
    pub fn try_days(days: i64) -> Option<Self> {
        Self::try_seconds(days.checked_mul(86_400)?)
    }

    /// 由整周构造；超出范围时返回 `None`。
    pub fn try_weeks(weeks: i64) -> Option<Self> {
        Self::try_seconds(weeks.checked_mul(604_800)?)
    }

    /// 由毫秒构造；`i64::MIN` 不在历史取值范围内，返回 `None`。
    pub fn try_milliseconds(milliseconds: i64) -> Option<Self> {
        (milliseconds != i64::MIN).then(|| Self::milliseconds(milliseconds))
    }

    /// 是否为零。
    pub const fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// 整周数，朝零截断。
    pub const fn num_weeks(&self) -> i64 {
        self.num_days() / 7
    }

    /// 整天数，朝零截断。
    pub const fn num_days(&self) -> i64 {
        self.0.as_secs() / 86_400
    }

    /// 整小时数，朝零截断。
    pub const fn num_hours(&self) -> i64 {
        self.0.as_secs() / 3_600
    }

    /// 整分钟数，朝零截断。
    pub const fn num_minutes(&self) -> i64 {
        self.0.as_secs() / 60
    }

    /// 整秒数，朝零截断。
    pub const fn num_seconds(&self) -> i64 {
        self.0.as_secs()
    }

    /// 整毫秒数，朝零截断；超出 `i64` 时饱和。
    pub fn num_milliseconds(&self) -> i64 {
        saturating_i64(self.0.as_millis())
    }

    /// 整微秒数，朝零截断；超出 `i64` 时返回 `None`。
    pub fn num_microseconds(&self) -> Option<i64> {
        i64::try_from(self.0.as_micros()).ok()
    }

    /// 纳秒数；超出 `i64` 时返回 `None`。
    pub fn num_nanoseconds(&self) -> Option<i64> {
        i64::try_from(self.0.as_nanos()).ok()
    }

    /// 不足一秒的部分，纳秒；符号与整体一致。
    pub const fn subsec_nanos(&self) -> i32 {
        self.0.subsec_nanos()
    }

    /// 绝对值；最小值取绝对值时饱和。
    pub fn abs(&self) -> Self {
        if self.0.is_negative() {
            Self(self.0.checked_neg().unwrap_or(SignedDuration::MAX))
        } else {
            *self
        }
    }

    /// 相加，溢出时返回 `None`。
    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    /// 相减，溢出时返回 `None`。
    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    /// 转成标准库时长；负值返回错误。
    pub fn to_std(&self) -> Result<std::time::Duration, OutOfRangeError> {
        std::time::Duration::try_from(self.0).ok().ok_or(OutOfRangeError)
    }

    /// 由标准库时长构造；超出范围返回错误。
    pub fn from_std(duration: std::time::Duration) -> Result<Self, OutOfRangeError> {
        let seconds = i64::try_from(duration.as_secs()).ok();
        let nanoseconds = i32::try_from(duration.subsec_nanos()).ok();
        seconds
            .zip(nanoseconds)
            .and_then(|(seconds, nanoseconds)| Self::from_floor_parts(seconds, nanoseconds))
            .ok_or(OutOfRangeError)
    }

    /// 除以整数，除数为零时返回 `None`；取整方式与 `/` 相同。
    pub fn checked_div(&self, divisor: i32) -> Option<Self> {
        (divisor != 0).then(|| *self / divisor)
    }

    /// 底层的 jiff 时长，供日历与时区运算使用。
    pub const fn to_jiff(self) -> SignedDuration {
        self.0
    }

    /// 由“向下取整的秒 + 非负纳秒”构造，只接受历史取值范围：毫秒数的绝对值不超过 `i64::MAX`。
    fn from_floor_parts(seconds: i64, nanoseconds: i32) -> Option<Self> {
        const MAX_SECONDS: i64 = i64::MAX / 1_000;
        const MAX_NANOSECONDS: i32 = 807_000_000;
        const MIN_SECONDS: i64 = -i64::MAX / 1_000 - 1;
        const MIN_NANOSECONDS: i32 = 193_000_000;
        let in_range = (0..1_000_000_000).contains(&nanoseconds)
            && (MIN_SECONDS..=MAX_SECONDS).contains(&seconds)
            && !(seconds == MAX_SECONDS && nanoseconds > MAX_NANOSECONDS)
            && !(seconds == MIN_SECONDS && nanoseconds < MIN_NANOSECONDS);
        in_range.then(|| {
            Self(
                SignedDuration::from_secs(seconds)
                    + SignedDuration::from_nanos(i64::from(nanoseconds)),
            )
        })
    }

    /// 规范化成“向下取整的秒 + 非负纳秒”，供序列化、`Debug` 与 `Display` 使用。
    fn floor_parts(&self) -> (i64, i32) {
        let (seconds, nanoseconds) = (self.0.as_secs(), self.0.subsec_nanos());
        if nanoseconds < 0 {
            (seconds - 1, nanoseconds + 1_000_000_000)
        } else {
            (seconds, nanoseconds)
        }
    }
}

fn saturating_i64(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

impl From<SignedDuration> for Duration {
    fn from(duration: SignedDuration) -> Self {
        Self(duration)
    }
}

impl From<Duration> for SignedDuration {
    fn from(duration: Duration) -> Self {
        duration.0
    }
}

impl Add for Duration {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

impl Sub for Duration {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0)
    }
}

impl AddAssign for Duration {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl SubAssign for Duration {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl Neg for Duration {
    type Output = Self;

    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Mul<i32> for Duration {
    type Output = Self;

    fn mul(self, factor: i32) -> Self {
        Self(self.0 * factor)
    }
}

impl Div<i32> for Duration {
    type Output = Self;

    /// 秒与纳秒分别朝零整除、再合并，与历史实现逐纳秒一致（与精确除法最多相差 1 纳秒）。
    ///
    /// # Panics
    ///
    /// 除数为零时 panic，与整数除法一致。
    fn div(self, divisor: i32) -> Self {
        let (seconds, nanoseconds) = self.floor_parts();
        let wide_divisor = i64::from(divisor);
        let quotient = seconds / wide_divisor;
        let carry = seconds % wide_divisor;
        let extra = carry * 1_000_000_000 / wide_divisor;
        let remainder = i64::from(nanoseconds / divisor) + extra;
        Self(SignedDuration::from_secs(quotient) + SignedDuration::from_nanos(remainder))
    }
}

impl std::iter::Sum for Duration {
    fn sum<I: Iterator<Item = Self>>(durations: I) -> Self {
        durations.fold(Self::ZERO, |total, duration| total + duration)
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (seconds, nanoseconds) = self.floor_parts();
        formatter
            .debug_struct("TimeDelta")
            .field("secs", &seconds)
            .field("nanos", &nanoseconds)
            .finish()
    }
}

impl fmt::Display for Duration {
    /// ISO 8601 时长，只用秒表示：`PT90061.5S`、`-PT2S`、`P0D`；小数位去掉末尾的零。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (magnitude, sign) = if self.0.is_negative() { (self.abs(), "-") } else { (*self, "") };
        let (seconds, nanoseconds) = magnitude.floor_parts();
        write!(formatter, "{sign}P")?;
        if seconds == 0 && nanoseconds == 0 {
            return formatter.write_str("0D");
        }
        write!(formatter, "T{seconds}")?;
        if nanoseconds > 0 {
            let fraction = format!("{nanoseconds:09}");
            write!(formatter, ".{}", fraction.trim_end_matches('0'))?;
        }
        formatter.write_str("S")
    }
}
