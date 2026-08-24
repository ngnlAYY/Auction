use std::fmt::Write as _;

use auction_application::RecurringSchedule;
use chrono::{DateTime, Local, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::Cron;
use thiserror::Error;

const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;
const SIGNED_DURATION_LIMIT: u128 = 1_u128 << 63;

const MONTH_NAMES: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];
const WEEKDAY_NAMES: &[(&str, u32)] =
    &[("sun", 0), ("mon", 1), ("tue", 2), ("wed", 3), ("thu", 4), ("fri", 5), ("sat", 6)];

/// 与旧版 `cron.ParseStandard` 对齐的五字段 cron 调度。
///
/// 支持标准 minute/hour/day-of-month/month/day-of-week 字段、英文月份与星期缩写、旧版描述符、
/// `@every` Go duration，以及 `TZ=` / `CRON_TZ=` IANA 时区前缀。未声明时区时沿用进程本地
/// 时区；对外只暴露 UTC 触发时刻。
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
    /// 解析旧版接受的标准五字段 cron、描述符和时区前缀。
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
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    let bounds = [
        FieldBounds::new(0, 59, &[]),
        FieldBounds::new(0, 23, &[]),
        FieldBounds::new(1, 31, &[]),
        FieldBounds::new(1, 12, MONTH_NAMES),
        FieldBounds::new(0, 6, WEEKDAY_NAMES),
    ];
    let mut canonical = String::new();
    for (index, (field, field_bounds)) in fields.iter().zip(bounds).enumerate() {
        if index > 0 {
            canonical.push(' ');
        }
        canonical.push_str(&parse_field(field, field_bounds)?.canonical(field_bounds));
    }
    let cron =
        canonical.parse::<Cron>().map_err(|_| StandardCronScheduleError::InvalidExpression)?;
    Ok(StandardCronKind::Calendar { cron: Box::new(cron), timezone })
}

#[derive(Debug, Clone, Copy)]
struct FieldBounds {
    minimum: u32,
    maximum: u32,
    names: &'static [(&'static str, u32)],
}

impl FieldBounds {
    const fn new(minimum: u32, maximum: u32, names: &'static [(&'static str, u32)]) -> Self {
        Self { minimum, maximum, names }
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedField {
    bits: u64,
    wildcard: bool,
}

impl ParsedField {
    fn canonical(self, bounds: FieldBounds) -> String {
        if self.wildcard {
            return "*".to_owned();
        }
        let mut value = String::new();
        for current in bounds.minimum..=bounds.maximum {
            if self.bits & (1_u64 << current) == 0 {
                continue;
            }
            if !value.is_empty() {
                value.push(',');
            }
            let _ = write!(value, "{current}");
        }
        value
    }
}

fn parse_field(value: &str, bounds: FieldBounds) -> Result<ParsedField, StandardCronScheduleError> {
    let mut bits = 0_u64;
    let mut wildcard = false;
    let mut found_expression = false;
    // Go 的 strings.FieldsFunc 会忽略连续、开头和结尾的逗号；这里保留相同接受边界。
    for expression in value.split(',').filter(|expression| !expression.is_empty()) {
        found_expression = true;
        let parsed = parse_range(expression, bounds)?;
        bits |= parsed.bits;
        wildcard |= parsed.wildcard;
    }
    if !found_expression || bits == 0 {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    Ok(ParsedField { bits, wildcard })
}

fn parse_range(
    expression: &str,
    bounds: FieldBounds,
) -> Result<ParsedField, StandardCronScheduleError> {
    let mut range_and_step = expression.split('/');
    let range = range_and_step.next().unwrap_or_default();
    let step_text = range_and_step.next();
    if range_and_step.next().is_some() {
        return Err(StandardCronScheduleError::InvalidExpression);
    }

    let mut low_and_high = range.split('-');
    let low = low_and_high.next().unwrap_or_default();
    let high = low_and_high.next();
    let too_many_hyphens = low_and_high.next().is_some();
    let single_value = high.is_none();

    let expression_wildcard = matches!(low, "*" | "?");
    let (start, mut end) = if expression_wildcard {
        // 旧版 wildcard 分支忽略后续 high；这也覆盖 `?` 与 `*/step`。
        (bounds.minimum, bounds.maximum)
    } else {
        if too_many_hyphens {
            return Err(StandardCronScheduleError::InvalidExpression);
        }
        let start = parse_field_value(low, bounds)?;
        let end = match high {
            Some(high) => parse_field_value(high, bounds)?,
            None => start,
        };
        (start, end)
    };

    let step = match step_text {
        Some(step) => parse_unsigned(step)?,
        None => 1,
    };
    if step_text.is_some() && single_value {
        end = bounds.maximum;
    }
    if start < bounds.minimum || end > bounds.maximum || start > end || step == 0 {
        return Err(StandardCronScheduleError::InvalidExpression);
    }

    let mut bits = 0_u64;
    for value in (start..=end).step_by(step as usize) {
        bits |= 1_u64 << value;
    }
    Ok(ParsedField { bits, wildcard: expression_wildcard && step == 1 })
}

fn parse_field_value(value: &str, bounds: FieldBounds) -> Result<u32, StandardCronScheduleError> {
    if !bounds.names.is_empty() {
        let normalized = value.to_ascii_lowercase();
        if let Some((_, number)) = bounds.names.iter().find(|(name, _)| *name == normalized) {
            return Ok(*number);
        }
    }
    parse_unsigned(value)
}

fn parse_unsigned(value: &str) -> Result<u32, StandardCronScheduleError> {
    let parsed = value.parse::<i64>().map_err(|_| StandardCronScheduleError::InvalidExpression)?;
    u32::try_from(parsed).map_err(|_| StandardCronScheduleError::InvalidExpression)
}

fn parse_every_delay(value: &str) -> Result<u64, StandardCronScheduleError> {
    let (negative, nanoseconds) = parse_go_duration(value)?;
    if negative || nanoseconds < NANOSECONDS_PER_SECOND {
        return Ok(1);
    }
    u64::try_from(nanoseconds / NANOSECONDS_PER_SECOND)
        .map_err(|_| StandardCronScheduleError::InvalidExpression)
}

/// 只解析旧版 `time.ParseDuration` 接受的单位和组合；调度随后按旧版规则截断到整秒。
fn parse_go_duration(value: &str) -> Result<(bool, u128), StandardCronScheduleError> {
    let mut remaining = value;
    let mut negative = false;
    if let Some(first) = remaining.as_bytes().first()
        && matches!(*first, b'-' | b'+')
    {
        negative = *first == b'-';
        remaining = &remaining[1..];
    }
    if remaining == "0" {
        return Ok((negative, 0));
    }
    if remaining.is_empty() {
        return Err(StandardCronScheduleError::InvalidExpression);
    }

    let mut total = 0_u128;
    while !remaining.is_empty() {
        let first = remaining.as_bytes()[0];
        if first != b'.' && !first.is_ascii_digit() {
            return Err(StandardCronScheduleError::InvalidExpression);
        }

        let (integer_digits, after_integer) = take_ascii_digits(remaining);
        let has_integer = !integer_digits.is_empty();
        let integer = parse_duration_integer(integer_digits)?;
        remaining = after_integer;

        let mut fraction = 0_u64;
        let mut scale = 1_f64;
        let mut has_fraction = false;
        if remaining.starts_with('.') {
            let (fraction_digits, after_fraction) = take_ascii_digits(&remaining[1..]);
            has_fraction = !fraction_digits.is_empty();
            (fraction, scale) = parse_duration_fraction(fraction_digits);
            remaining = after_fraction;
        }
        if !has_integer && !has_fraction {
            return Err(StandardCronScheduleError::InvalidExpression);
        }

        let unit_end = remaining
            .char_indices()
            .find_map(|(index, character)| {
                (character == '.' || character.is_ascii_digit()).then_some(index)
            })
            .unwrap_or(remaining.len());
        if unit_end == 0 {
            return Err(StandardCronScheduleError::InvalidExpression);
        }
        let unit = duration_unit_nanoseconds(&remaining[..unit_end])?;
        remaining = &remaining[unit_end..];

        if integer > SIGNED_DURATION_LIMIT / unit {
            return Err(StandardCronScheduleError::InvalidExpression);
        }
        let mut component = integer * unit;
        if fraction > 0 {
            component = component
                .checked_add((fraction as f64 * (unit as f64 / scale)) as u128)
                .ok_or(StandardCronScheduleError::InvalidExpression)?;
        }
        total = total
            .checked_add(component)
            .filter(|total| *total <= SIGNED_DURATION_LIMIT)
            .ok_or(StandardCronScheduleError::InvalidExpression)?;
    }
    if !negative && total > i64::MAX as u128 {
        return Err(StandardCronScheduleError::InvalidExpression);
    }
    Ok((negative, total))
}

fn take_ascii_digits(value: &str) -> (&str, &str) {
    let end =
        value.as_bytes().iter().position(|byte| !byte.is_ascii_digit()).unwrap_or(value.len());
    value.split_at(end)
}

fn parse_duration_integer(value: &str) -> Result<u128, StandardCronScheduleError> {
    let mut number = 0_u128;
    for digit in value.bytes() {
        if number > SIGNED_DURATION_LIMIT / 10 {
            return Err(StandardCronScheduleError::InvalidExpression);
        }
        number = number * 10 + u128::from(digit - b'0');
        if number > SIGNED_DURATION_LIMIT {
            return Err(StandardCronScheduleError::InvalidExpression);
        }
    }
    Ok(number)
}

fn parse_duration_fraction(value: &str) -> (u64, f64) {
    let mut fraction = 0_u64;
    let mut scale = 1_f64;
    let mut overflow = false;
    for digit in value.bytes() {
        if overflow {
            continue;
        }
        if fraction > (i64::MAX as u64) / 10 {
            overflow = true;
            continue;
        }
        let next = fraction * 10 + u64::from(digit - b'0');
        if u128::from(next) > SIGNED_DURATION_LIMIT {
            overflow = true;
            continue;
        }
        fraction = next;
        scale *= 10_f64;
    }
    (fraction, scale)
}

fn duration_unit_nanoseconds(value: &str) -> Result<u128, StandardCronScheduleError> {
    match value {
        "ns" => Ok(1),
        "us" | "µs" | "μs" => Ok(1_000),
        "ms" => Ok(1_000_000),
        "s" => Ok(NANOSECONDS_PER_SECOND),
        "m" => Ok(60 * NANOSECONDS_PER_SECOND),
        "h" => Ok(60 * 60 * NANOSECONDS_PER_SECOND),
        _ => Err(StandardCronScheduleError::InvalidExpression),
    }
}
