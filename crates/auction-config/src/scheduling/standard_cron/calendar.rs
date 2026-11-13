//! 五字段 cron 的日历匹配与下一次触发计算。

use chrono::{
    DateTime, Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Timelike,
    Utc, offset::LocalResult,
};

use super::fields::ParsedField;

/// 最稀疏的合法表达式是 2 月 29 日，相邻两次最多相隔 8 年；超出该范围视为永不触发。
const SEARCH_HORIZON_DAYS: u64 = 8 * 366;
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
    pub(super) fn next_after<Tz: TimeZone>(
        &self,
        now: DateTime<Utc>,
        timezone: &Tz,
    ) -> Option<DateTime<Utc>> {
        let local_now = now.with_timezone(timezone).naive_local();
        let horizon = local_now.date().checked_add_days(Days::new(SEARCH_HORIZON_DAYS))?;
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

    fn first_instant_after<Tz: TimeZone>(
        &self,
        now: DateTime<Utc>,
        local_now: NaiveDateTime,
        horizon: NaiveDate,
        timezone: &Tz,
        fixed_time: bool,
    ) -> Option<DateTime<Utc>> {
        let mut cursor = local_now;
        loop {
            let civil = self.next_civil_after(cursor, horizon)?;
            cursor = civil;
            match timezone.from_local_datetime(&civil) {
                LocalResult::Single(instant) => {
                    if instant > now {
                        return Some(instant.with_timezone(&Utc));
                    }
                }
                LocalResult::Ambiguous(earlier, later) => {
                    if earlier > now {
                        return Some(earlier.with_timezone(&Utc));
                    }
                    if !fixed_time && later > now {
                        return Some(later.with_timezone(&Utc));
                    }
                }
                LocalResult::None => {
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

    fn first_repeated_instant<Tz: TimeZone>(
        &self,
        now: DateTime<Utc>,
        local_now: NaiveDateTime,
        horizon: NaiveDate,
        timezone: &Tz,
    ) -> Option<DateTime<Utc>> {
        let LocalResult::Ambiguous(earlier, later) = timezone.from_local_datetime(&local_now)
        else {
            return None;
        };
        if later <= now {
            return None;
        }
        // 重复区间的起点晚于“当前墙上时间减去回拨量”，从该处起扫描即可覆盖整个后一次。
        let mut cursor = local_now.checked_sub_signed(later.signed_duration_since(&earlier))?;
        loop {
            let civil = self.next_civil_after(cursor, horizon)?;
            if civil > local_now {
                return None;
            }
            cursor = civil;
            if let LocalResult::Ambiguous(_, repeated) = timezone.from_local_datetime(&civil)
                && repeated > now
            {
                return Some(repeated.with_timezone(&Utc));
            }
        }
    }

    /// 返回严格晚于 `after` 的首个匹配墙上时间（秒恒为 0）；不考虑时区。
    fn next_civil_after(&self, after: NaiveDateTime, horizon: NaiveDate) -> Option<NaiveDateTime> {
        let mut date = after.date();
        if self.date_matches(date)
            && let Some(time) = self.next_time_after(after.time())
        {
            return Some(date.and_time(time));
        }
        let first_time = NaiveTime::from_hms_opt(self.hours.first()?, self.minutes.first()?, 0)?;
        loop {
            date = date.succ_opt()?;
            if date > horizon {
                return None;
            }
            if self.date_matches(date) {
                return Some(date.and_time(first_time));
            }
        }
    }

    /// 当日内严格晚于 `time` 的首个匹配时分；秒恒为 0，所以同一分钟永远不算更晚。
    fn next_time_after(&self, time: NaiveTime) -> Option<NaiveTime> {
        let (hour, minute) = (time.hour(), time.minute());
        if self.hours.contains(hour)
            && let Some(next_minute) = self.minutes.first_at_or_after(minute + 1)
        {
            return NaiveTime::from_hms_opt(hour, next_minute, 0);
        }
        let next_hour = self.hours.first_at_or_after(hour + 1)?;
        NaiveTime::from_hms_opt(next_hour, self.minutes.first()?, 0)
    }

    /// 日与星期任一字段为通配时只看另一字段；两者都受限时按传统 cron 取并集。
    fn date_matches(&self, date: NaiveDate) -> bool {
        if !self.months.contains(date.month()) {
            return false;
        }
        let day_of_month = self.days_of_month.contains(date.day());
        let day_of_week = self.days_of_week.contains(date.weekday().num_days_from_sunday());
        match (self.days_of_month.is_wildcard(), self.days_of_week.is_wildcard()) {
            (true, true) => true,
            (true, false) => day_of_week,
            (false, true) => day_of_month,
            (false, false) => day_of_month || day_of_week,
        }
    }
}

/// 缺口内的墙上时间都无法解析；二分找出其后首个可解析的秒及对应的真实时刻。
fn first_instant_after_gap<Tz: TimeZone>(
    timezone: &Tz,
    gap_civil: NaiveDateTime,
) -> Option<(NaiveDateTime, DateTime<Utc>)> {
    let resolve = |seconds: i64| {
        let civil = gap_civil.checked_add_signed(TimeDelta::seconds(seconds))?;
        match timezone.from_local_datetime(&civil) {
            LocalResult::Single(instant) | LocalResult::Ambiguous(instant, _) => {
                Some((civil, instant.with_timezone(&Utc)))
            }
            LocalResult::None => None,
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
