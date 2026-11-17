//! 按 UTC 日历字段调整时刻：取整、改写秒与纳秒、按月推进，以及 strftime 格式化。

use jiff::civil;
use jiff::fmt::strtime;

use super::Timestamp;
use crate::duration::Duration;

impl Timestamp {
    /// 把 UTC 秒字段改写为 `second`（0 到 59），其余字段不变。
    pub fn with_second(self, second: u32) -> Option<Self> {
        let second = i8::try_from(second).ok()?;
        Self::from_utc_datetime(self.to_utc_datetime().with().second(second).build().ok()?)
    }

    /// 把秒内纳秒改写为 `nanosecond`（须小于一秒），其余字段不变。
    pub fn with_nanosecond(self, nanosecond: u32) -> Option<Self> {
        let nanosecond = i32::try_from(nanosecond).ok()?;
        Self::from_utc_datetime(
            self.to_utc_datetime().with().subsec_nanosecond(nanosecond).build().ok()?,
        )
    }

    /// 向下取整到自 Unix 纪元起 `duration` 的整数倍。
    ///
    /// `duration` 须为正且不超过 `i64` 纳秒；时刻超出 `i64` 纳秒范围（约 1677–2262 年之外）时
    /// 返回 `None`，由调用方决定退回原值还是拒绝。
    pub fn duration_trunc(self, duration: Duration) -> Option<Self> {
        let span = duration.num_nanoseconds().filter(|span| *span > 0)?;
        let remainder = self.timestamp_nanos_opt()?.rem_euclid(span);
        self.checked_sub_signed(Duration::nanoseconds(remainder))
    }

    /// 截去秒内小数，只保留 `digits` 位；9 位及以上原样返回。
    #[must_use]
    pub fn trunc_subsecs(self, digits: u16) -> Self {
        let span = match digits {
            0 => 1_000_000_000,
            1 => 100_000_000,
            2 => 10_000_000,
            3 => 1_000_000,
            4 => 100_000,
            5 => 10_000,
            6 => 1_000,
            7 => 100,
            8 => 10,
            _ => 1,
        };
        let remainder = self.timestamp_subsec_nanos() % span;
        self.checked_sub_signed(Duration::nanoseconds(i64::from(remainder))).unwrap_or(self)
    }

    /// 按 UTC 日历推进若干个月：时分秒不变，目标月没有同一天时取该月最后一天。
    pub fn checked_add_months(self, months: u32) -> Option<Self> {
        let span = jiff::Span::new().try_months(i64::from(months)).ok()?;
        Self::from_utc_datetime(self.to_utc_datetime().checked_add(span).ok()?)
    }

    /// 按 strptime 模式解析不带时区的日期时间，并把它当作 UTC。
    ///
    /// 文本里的星期与日期不符、字段缺失或有多余内容时返回 `None`。
    pub fn strptime_utc(format: &str, text: &str) -> Option<Self> {
        Self::from_utc_datetime(civil::DateTime::strptime(format, text).ok()?)
    }

    /// 按 strftime 模式格式化 UTC 下的日期与时间字段。
    ///
    /// 模式须为调用方写死的合法字面量：模式无效时格式化返回 `fmt::Error`。
    pub fn strftime(self, format: &str) -> strtime::Display<'_> {
        self.0.strftime(format)
    }
}
