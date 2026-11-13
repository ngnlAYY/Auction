//! 五字段 cron 的名称、范围和位集合解析。

use super::StandardCronScheduleError;

pub(super) const MONTH_NAMES: &[(&str, u32)] = &[
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

pub(super) const WEEKDAY_NAMES: &[(&str, u32)] =
    &[("sun", 0), ("mon", 1), ("tue", 2), ("wed", 3), ("thu", 4), ("fri", 5), ("sat", 6)];

#[derive(Debug, Clone, Copy)]
pub(super) struct FieldBounds {
    minimum: u32,
    maximum: u32,
    names: &'static [(&'static str, u32)],
}

impl FieldBounds {
    pub(super) const fn new(
        minimum: u32,
        maximum: u32,
        names: &'static [(&'static str, u32)],
    ) -> Self {
        Self { minimum, maximum, names }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ParsedField {
    bits: u64,
    wildcard: bool,
}

impl ParsedField {
    pub(super) const fn is_wildcard(self) -> bool {
        self.wildcard
    }

    pub(super) const fn contains(self, value: u32) -> bool {
        value < u64::BITS && self.bits & (1_u64 << value) != 0
    }

    /// 最小的取值；解析保证至少有一个取值。
    pub(super) const fn first(self) -> Option<u32> {
        self.first_at_or_after(0)
    }

    /// 不小于 `minimum` 的最小取值。
    pub(super) const fn first_at_or_after(self, minimum: u32) -> Option<u32> {
        if minimum >= u64::BITS {
            return None;
        }
        let remaining = self.bits & (u64::MAX << minimum);
        if remaining == 0 { None } else { Some(remaining.trailing_zeros()) }
    }

    /// 字段只有一个取值时返回它；通配字段总有多个取值。
    pub(super) const fn single_value(self) -> Option<u32> {
        if self.bits.is_power_of_two() { Some(self.bits.trailing_zeros()) } else { None }
    }
}

pub(super) fn parse_field(
    value: &str,
    bounds: FieldBounds,
) -> Result<ParsedField, StandardCronScheduleError> {
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
    if let Some((_, number)) =
        bounds.names.iter().find(|(name, _)| name.eq_ignore_ascii_case(value))
    {
        return Ok(*number);
    }
    parse_unsigned(value)
}

fn parse_unsigned(value: &str) -> Result<u32, StandardCronScheduleError> {
    let parsed = value.parse::<i64>().map_err(|_| StandardCronScheduleError::InvalidExpression)?;
    u32::try_from(parsed).map_err(|_| StandardCronScheduleError::InvalidExpression)
}
