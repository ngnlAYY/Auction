//! 五字段标准 cron 调度的解析与下一次触发计算。

use auction_application::scheduling::RecurringSchedule;
use auction_time::{TimeDelta, Timestamp};
use jiff::tz::TimeZone;
use serde::Serialize;
use thiserror::Error;

mod calendar;
mod duration;
mod fields;

use calendar::CalendarSchedule;
use duration::parse_every_delay;
use fields::{FieldBounds, MONTH_NAMES, WEEKDAY_NAMES, parse_field};

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;
const SIGNED_DURATION_LIMIT: u128 = 1_u128 << 63;

/// 保留旧版 `cron.ParseStandard` 输入语法的五字段 cron 调度。
/// 支持标准 minute/hour/day-of-month/month/day-of-week 字段、英文月份与星期缩写、描述符、
/// `@every` Go duration，以及 `TZ=` / `CRON_TZ=` IANA 时区前缀。未声明时区时沿用进程本地
/// 时区；日历触发及夏令时选择由 `calendar` 模块持有，对外只暴露 UTC 触发时刻。
#[derive(Debug, Clone)]
pub struct StandardCronSchedule {
    kind: StandardCronKind,
}

#[derive(Debug, Clone)]
enum StandardCronKind {
    Calendar { calendar: CalendarSchedule, timezone: ScheduleTimezone },
    Every { delay_seconds: u64 },
}

#[derive(Debug, Clone)]
enum ScheduleTimezone {
    Local,
    Utc,
    Named(TimeZone),
}

/// 标准 cron 配置错误；错误不回显原始配置文本，避免把未来扩展字段带入日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StandardCronScheduleError {
    #[error("standard cron expression is invalid")]
    InvalidExpression,
    #[error("standard cron timezone is invalid")]
    InvalidTimezone,
}

impl StandardCronSchedule {
    /// 解析兼容的标准五字段 cron、描述符和时区前缀，不承诺外部实现的日历触发语义。
    pub fn parse(value: &str) -> Result<Self, StandardCronScheduleError> {
        let (timezone, expression) = split_timezone(value)?;
        let kind = match expression {
            "@yearly" | "@annually" => calendar_kind("0 0 1 1 *", timezone)?,
            "@monthly" => calendar_kind("0 0 1 * *", timezone)?,
            "@weekly" => calendar_kind("0 0 * * 0", timezone)?,
            "@daily" | "@midnight" => calendar_kind("0 0 * * *", timezone)?,
            "@hourly" => calendar_kind("0 * * * *", timezone)?,
            descriptor if descriptor.starts_with("@every ") => {
                StandardCronKind::Every { delay_seconds: parse_every_delay(&descriptor[7..])? }
            }
            descriptor if descriptor.starts_with('@') => {
                return Err(StandardCronScheduleError::InvalidExpression);
            }
            expression => calendar_kind(expression, timezone)?,
        };
        Ok(Self { kind })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CronProjection {
    Every {
        delay_seconds: u64,
    },
    Calendar {
        timezone: String,
        minutes: u64,
        hours: u64,
        days_of_month: u64,
        months: u64,
        days_of_week: u64,
    },
}

impl StandardCronSchedule {
    /// 按已解析的匹配语义投影；不读取系统当前时区或采样下一次触发时间。
    pub(crate) fn cold_projection(&self) -> impl Serialize + use<> {
        match &self.kind {
            StandardCronKind::Every { delay_seconds } => {
                CronProjection::Every { delay_seconds: *delay_seconds }
            }
            StandardCronKind::Calendar { calendar, timezone } => {
                let timezone = match timezone {
                    ScheduleTimezone::Local => "Local",
                    ScheduleTimezone::Utc => "UTC",
                    ScheduleTimezone::Named(zone) => zone.iana_name().unwrap_or("UTC"),
                };
                // 日字段取并集；通配时按原匹配器忽略该字段，空的规范键表示每日都匹配。
                let day_of_month = calendar.days_of_month;
                let day_of_week = calendar.days_of_week;
                let (days_of_month, days_of_week) =
                    match (day_of_month.is_wildcard(), day_of_week.is_wildcard()) {
                        (true, true) => (0, 0),
                        (true, false) => (0, day_of_week.bits()),
                        (false, true) => (day_of_month.bits(), 0),
                        (false, false) => (day_of_month.bits(), day_of_week.bits()),
                    };
                let (days_of_month, days_of_week) =
                    if days_of_month == u64::from(u32::MAX - 1) || days_of_week == 0b111_1111 {
                        (0, 0)
                    } else {
                        (days_of_month, days_of_week)
                    };
                CronProjection::Calendar {
                    timezone: timezone.to_owned(),
                    minutes: calendar.minutes.bits(),
                    hours: calendar.hours.bits(),
                    days_of_month,
                    months: calendar.months.bits(),
                    days_of_week,
                }
            }
        }
    }
}

impl RecurringSchedule for StandardCronSchedule {
    fn next_after(&self, now: Timestamp) -> Option<Timestamp> {
        let next = match &self.kind {
            StandardCronKind::Calendar { calendar, timezone: ScheduleTimezone::Local } => {
                // 进程本地时区每次触发计算时读取，主机改时区后不必重启。
                calendar.next_after(now, &TimeZone::system())?
            }
            StandardCronKind::Calendar { calendar, timezone: ScheduleTimezone::Utc } => {
                calendar.next_after(now, &TimeZone::UTC)?
            }
            StandardCronKind::Calendar {
                calendar,
                timezone: ScheduleTimezone::Named(timezone),
            } => calendar.next_after(now, timezone)?,
            StandardCronKind::Every { delay_seconds } => {
                let delay = TimeDelta::seconds(i64::try_from(*delay_seconds).ok()?);
                let fractional_second =
                    TimeDelta::nanoseconds(i64::from(now.timestamp_subsec_nanos()));
                now.checked_add_signed(delay)?.checked_sub_signed(fractional_second)?
            }
        };
        (next > now).then_some(next)
    }
}

fn split_timezone(value: &str) -> Result<(ScheduleTimezone, &str), StandardCronScheduleError> {
    let prefix_length = if value.starts_with("TZ=") {
        Some(3)
    } else if value.starts_with("CRON_TZ=") {
        Some(8)
    } else {
        None
    };
    let Some(prefix_length) = prefix_length else {
        return Ok((ScheduleTimezone::Local, value));
    };
    // 旧版解析器只把第一个 ASCII 空格识别为时区前缀结束位置。
    let separator = value.find(' ').ok_or(StandardCronScheduleError::InvalidTimezone)?;
    let timezone_name = &value[prefix_length..separator];
    if timezone_name.is_empty() {
        return Err(StandardCronScheduleError::InvalidTimezone);
    }
    let timezone = match timezone_name {
        "Local" => ScheduleTimezone::Local,
        "UTC" => ScheduleTimezone::Utc,
        name => ScheduleTimezone::Named(
            auction_domain::parse_timezone(name)
                .ok_or(StandardCronScheduleError::InvalidTimezone)?,
        ),
    };
    let expression = value[separator..].trim();
    if expression.is_empty() {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    Ok((timezone, expression))
}

fn calendar_kind(
    expression: &str,
    timezone: ScheduleTimezone,
) -> Result<StandardCronKind, StandardCronScheduleError> {
    let mut fields = expression.split_whitespace();
    let mut next_field = |bounds: FieldBounds| {
        parse_field(fields.next().ok_or(StandardCronScheduleError::InvalidExpression)?, bounds)
    };
    // 字段解析器持有旧版的接受边界（5/5、星期与月份别名、wildcard）；日历只消费展开后的取值集合。
    let calendar = CalendarSchedule {
        minutes: next_field(FieldBounds::new(0, 59, &[]))?,
        hours: next_field(FieldBounds::new(0, 23, &[]))?,
        days_of_month: next_field(FieldBounds::new(1, 31, &[]))?,
        months: next_field(FieldBounds::new(1, 12, MONTH_NAMES))?,
        days_of_week: next_field(FieldBounds::new(0, 6, WEEKDAY_NAMES))?,
    };
    if fields.next().is_some() {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    Ok(StandardCronKind::Calendar { calendar, timezone })
}

#[cfg(test)]
mod tests;
