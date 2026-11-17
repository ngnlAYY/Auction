//! 五字段 cron 的日历匹配与下一次触发计算。

use auction_time::{LocalTime, Timestamp};
use jiff::SignedDuration;
use jiff::ToSpan;
use jiff::civil;
use jiff::tz::TimeZone;

use super::fields::ParsedField;

/// 最稀疏的合法表达式是 2 月 29 日，相邻两次最多相隔 8 年；超出该范围视为永不触发。
const SEARCH_HORIZON_DAYS: i64 = 8 * 366;
/// 固定时刻落入夏令时缺口时，向后寻找首个真实时刻的最大跨度。
const MAXIMUM_GAP_SECONDS: i64 = 2 * 60 * 60;

/// 已展开的五字段日历表达式；只按墙上时间匹配，时区由调用方传入。
#[derive(Debug, Clone, Copy)]
pub(super) struct CalendarSchedule {
    pub(super) minutes: ParsedField,
    pub(super) hours: ParsedField,
    pub(super) days_of_month: ParsedField,
    pub(super) months: ParsedField,
    pub(super) days_of_week: ParsedField,
}

impl CalendarSchedule {
    /// 返回严格晚于 `now` 的首个触发时刻。
    ///
    /// 时、分各只有一个取值的表达式是固定时刻：落入夏令时缺口时改在缺口后的首个真实时刻
    /// 触发，回拨重复时只取较早的一次。其余表达式按真实时间顺序匹配墙上时间：缺口内的
    /// 时刻不存在，回拨重复的两次都触发。
    pub(super) fn next_after(&self, now: Timestamp, timezone: &TimeZone) -> Option<Timestamp> {
        let local_now = timezone.to_datetime(now.to_jiff());
        let horizon = local_now.date().checked_add(SEARCH_HORIZON_DAYS.days()).ok()?;
        let fixed_time =
            self.minutes.single_value().is_some() && self.hours.single_value().is_some();

        let forward = self.first_instant_after(now, local_now, horizon, timezone, fixed_time);
        if fixed_time {
            return forward;
        }
        // 墙上时间正处于回拨重复区间的前一次时，后一次会重现不晚于当前的墙上时间；
        // 向前的墙上时间搜索看不到它们，需单独取后一次中最早的匹配并与前者比较。
        let repeated = self.first_repeated_instant(now, local_now, horizon, timezone);
        match (forward, repeated) {
            (Some(forward), Some(repeated)) => Some(forward.min(repeated)),
            (forward, repeated) => forward.or(repeated),
        }
    }

    fn first_instant_after(
        &self,
        now: Timestamp,
        local_now: civil::DateTime,
        horizon: civil::Date,
        timezone: &TimeZone,
        fixed_time: bool,
    ) -> Option<Timestamp> {
        let mut cursor = local_now;
        loop {
            let civil = self.next_civil_after(cursor, horizon)?;
            cursor = civil;
            match LocalTime::resolve(timezone, civil) {
                LocalTime::Unique(instant) => {
                    if instant > now {
                        return Some(instant);
                    }
                }
                LocalTime::Repeated { earlier, later } => {
                    if earlier > now {
                        return Some(earlier);
                    }
                    if !fixed_time && later > now {
                        return Some(later);
                    }
                }
                LocalTime::Skipped { .. } => {
                    if fixed_time
                        && let Some((after_gap, instant)) = first_instant_after_gap(timezone, civil)
                        // 原定时刻不存在，只要求缺口后那一刻所在日期仍在日程内。
                        && self.date_matches(after_gap.date())
                        && instant > now
                    {
                        return Some(instant);
                    }
                }
            }
        }
    }

    fn first_repeated_instant(
        &self,
        now: Timestamp,
        local_now: civil::DateTime,
        horizon: civil::Date,
        timezone: &TimeZone,
    ) -> Option<Timestamp> {
        let LocalTime::Repeated { earlier, later } = LocalTime::resolve(timezone, local_now) else {
            return None;
        };
        if later <= now {
            return None;
        }
        // 重复区间的起点晚于“当前墙上时间减去回拨量”，从该处起扫描即可覆盖整个后一次。
        let mut cursor = local_now.checked_sub((later - earlier).to_jiff()).ok()?;
        loop {
            let civil = self.next_civil_after(cursor, horizon)?;
            if civil > local_now {
                return None;
            }
            cursor = civil;
            if let LocalTime::Repeated { later: repeated, .. } = LocalTime::resolve(timezone, civil)
                && repeated > now
            {
                return Some(repeated);
            }
        }
    }

    /// 返回严格晚于 `after` 的首个匹配墙上时间（秒恒为 0）；不考虑时区。
    fn next_civil_after(
        &self,
        after: civil::DateTime,
        horizon: civil::Date,
    ) -> Option<civil::DateTime> {
        let mut date = after.date();
        if self.date_matches(date)
            && let Some(time) = self.next_time_after(after.time())
        {
            return Some(date.to_datetime(time));
        }
        let first_time = clock(self.hours.first()?, self.minutes.first()?)?;
        loop {
            date = date.tomorrow().ok()?;
            if date > horizon {
                return None;
            }
            if self.date_matches(date) {
                return Some(date.to_datetime(first_time));
            }
        }
    }

    /// 当日内严格晚于 `time` 的首个匹配时分；秒恒为 0，所以同一分钟永远不算更晚。
    fn next_time_after(&self, time: civil::Time) -> Option<civil::Time> {
        let (hour, minute) = (field_value(time.hour()), field_value(time.minute()));
        if self.hours.contains(hour)
            && let Some(next_minute) = self.minutes.first_at_or_after(minute + 1)
        {
            return clock(hour, next_minute);
        }
        let next_hour = self.hours.first_at_or_after(hour + 1)?;
        clock(next_hour, self.minutes.first()?)
    }

    /// 日与星期任一字段为通配时只看另一字段；两者都受限时按传统 cron 取并集。
    fn date_matches(&self, date: civil::Date) -> bool {
        if !self.months.contains(field_value(date.month())) {
            return false;
        }
        let day_of_month = self.days_of_month.contains(field_value(date.day()));
        let day_of_week =
            self.days_of_week.contains(field_value(date.weekday().to_sunday_zero_offset()));
        match (self.days_of_month.is_wildcard(), self.days_of_week.is_wildcard()) {
            (true, true) => true,
            (true, false) => day_of_week,
            (false, true) => day_of_month,
            (false, false) => day_of_month || day_of_week,
        }
    }
}

/// 缺口内的墙上时间都无法解析；二分找出其后首个可解析的秒及对应的真实时刻。
fn first_instant_after_gap(
    timezone: &TimeZone,
    gap_civil: civil::DateTime,
) -> Option<(civil::DateTime, Timestamp)> {
    let resolve = |seconds: i64| {
        let civil = gap_civil.checked_add(SignedDuration::from_secs(seconds)).ok()?;
        match LocalTime::resolve(timezone, civil) {
            LocalTime::Unique(instant) | LocalTime::Repeated { earlier: instant, .. } => {
                Some((civil, instant))
            }
            LocalTime::Skipped { .. } => None,
        }
    };
    let mut inside_gap = 0;
    let mut outside_gap = MAXIMUM_GAP_SECONDS;
    let mut resolved = resolve(outside_gap)?;
    while outside_gap - inside_gap > 1 {
        let middle = inside_gap + (outside_gap - inside_gap) / 2;
        match resolve(middle) {
            Some(candidate) => {
                outside_gap = middle;
                resolved = candidate;
            }
            None => inside_gap = middle,
        }
    }
    Some(resolved)
}

/// 日历字段都是非负的小整数，转换为字段解析器使用的 `u32`。
fn field_value(value: i8) -> u32 {
    u32::from(value.unsigned_abs())
}

/// 由字段取值构造整分钟的钟点；取值超出一天的范围时返回 `None`。
fn clock(hour: u32, minute: u32) -> Option<civil::Time> {
    civil::Time::new(i8::try_from(hour).ok()?, i8::try_from(minute).ok()?, 0, 0).ok()
}
