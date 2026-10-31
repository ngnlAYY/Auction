use auction_application::RecurringSchedule;
use chrono::{DateTime, Local, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;
use thiserror::Error;

mod duration;
mod fields;

use duration::parse_every_delay;
use fields::{FieldBounds, MONTH_NAMES, WEEKDAY_NAMES, parse_field};

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;
const SIGNED_DURATION_LIMIT: u128 = 1_u128 << 63;

/// 保留旧版 `cron.ParseStandard` 输入语法的五字段 cron 调度。
/// 支持标准 minute/hour/day-of-month/month/day-of-week 字段、英文月份与星期缩写、描述符、
/// `@every` Go duration，以及 `TZ=` / `CRON_TZ=` IANA 时区前缀。未声明时区时沿用进程本地
/// 时区；日历触发及夏令时选择由 croner 持有，对外只暴露 UTC 触发时刻。
#[derive(Debug, Clone)]
pub struct StandardCronSchedule {
    kind: StandardCronKind,
}

#[derive(Debug, Clone)]
enum StandardCronKind {
    Calendar { cron: Box<Cron>, timezone: ScheduleTimezone },
    Every { delay_seconds: u64 },
}

#[derive(Debug, Clone)]
enum ScheduleTimezone {
    Local,
    Utc,
    Named(Tz),
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

impl RecurringSchedule for StandardCronSchedule {
    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let next = match &self.kind {
            StandardCronKind::Calendar { cron, timezone: ScheduleTimezone::Local } => cron
                .find_next_occurrence(&now.with_timezone(&Local), false)
                .ok()?
                .with_timezone(&Utc),
            StandardCronKind::Calendar { cron, timezone: ScheduleTimezone::Utc } => {
                cron.find_next_occurrence(&now, false).ok()?
            }
            StandardCronKind::Calendar { cron, timezone: ScheduleTimezone::Named(timezone) } => {
                cron.find_next_occurrence(&now.with_timezone(timezone), false)
                    .ok()?
                    .with_timezone(&Utc)
            }
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
            name.parse::<Tz>().map_err(|_| StandardCronScheduleError::InvalidTimezone)?,
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
    let bounds = [
        FieldBounds::new(0, 59, &[]),
        FieldBounds::new(0, 23, &[]),
        FieldBounds::new(1, 31, &[]),
        FieldBounds::new(1, 12, MONTH_NAMES),
        FieldBounds::new(0, 6, WEEKDAY_NAMES),
    ];
    let mut canonical = String::new();
    for (index, field_bounds) in bounds.into_iter().enumerate() {
        let field = fields.next().ok_or(StandardCronScheduleError::InvalidExpression)?;
        if index > 0 {
            canonical.push(' ');
        }
        parse_field(field, field_bounds)?.write_canonical(field_bounds, &mut canonical);
    }
    if fields.next().is_some() {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    // croner 4 默认拒绝单值步长；这里仅传入既有解析器展开后的数字列表或 *，
    // 保留旧版 5/5、星期/月份别名及 wildcard 的接受边界，不放宽底层解析器。
    let cron =
        canonical.parse::<Cron>().map_err(|_| StandardCronScheduleError::InvalidExpression)?;
    Ok(StandardCronKind::Calendar { cron: Box::new(cron), timezone })
}
