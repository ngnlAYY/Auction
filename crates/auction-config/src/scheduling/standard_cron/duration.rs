//! `@every` 和 Go duration 兼容解析。

use super::*;

pub(super) fn parse_every_delay(value: &str) -> Result<u64, StandardCronScheduleError> {
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
