//! 登录时长表单的精确单位换算；小数与科学计数法必须得到整分钟，不做浮点舍入。

use crate::api::valid_session_duration;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum DurationUnit {
    Minutes,
    Hours,
    #[default]
    Days,
}

impl DurationUnit {
    pub(super) fn from_value(value: &str) -> Option<Self> {
        match value {
            "minutes" => Some(Self::Minutes),
            "hours" => Some(Self::Hours),
            "days" => Some(Self::Days),
            _ => None,
        }
    }

    pub(super) fn value(self) -> &'static str {
        match self {
            Self::Minutes => "minutes",
            Self::Hours => "hours",
            Self::Days => "days",
        }
    }

    pub(super) fn maximum(self) -> &'static str {
        match self {
            Self::Minutes => "525600",
            Self::Hours => "8760",
            Self::Days => "365",
        }
    }

    fn minutes(self) -> u128 {
        match self {
            Self::Minutes => 1,
            Self::Hours => 60,
            Self::Days => 1_440,
        }
    }
}

pub(super) struct DurationDraft {
    pub(super) permanent: bool,
    pub(super) amount: String,
    pub(super) unit: DurationUnit,
}

impl Default for DurationDraft {
    fn default() -> Self {
        Self { permanent: true, amount: "1".to_owned(), unit: DurationUnit::Days }
    }
}

impl DurationDraft {
    pub(super) fn from_seconds(seconds: Option<i64>) -> Self {
        let Some(seconds) = seconds else { return Self::default() };
        let minutes = seconds / 60;
        let (amount, unit) = if minutes % 1_440 == 0 {
            (minutes / 1_440, DurationUnit::Days)
        } else if minutes % 60 == 0 {
            (minutes / 60, DurationUnit::Hours)
        } else {
            (minutes, DurationUnit::Minutes)
        };
        Self { permanent: false, amount: amount.to_string(), unit }
    }

    pub(super) fn seconds(&self) -> Result<Option<i64>, ()> {
        if self.permanent {
            return Ok(None);
        }
        let minutes = exact_minutes(&self.amount, self.unit).ok_or(())?;
        let seconds = i64::try_from(minutes).ok().and_then(|value| value.checked_mul(60));
        match seconds {
            Some(seconds) if valid_session_duration(Some(seconds)) => Ok(Some(seconds)),
            _ => Err(()),
        }
    }
}

fn exact_minutes(amount: &str, unit: DurationUnit) -> Option<u128> {
    if amount.is_empty() || amount.len() > 64 {
        return None;
    }
    let (mantissa, exponent) = match amount.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (amount, 0),
    };
    let mut numerator = 0_u128;
    let mut fraction_digits = 0_i32;
    let mut decimal = false;
    let mut digits = false;
    for byte in mantissa.bytes() {
        if byte == b'.' && !decimal {
            decimal = true;
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        digits = true;
        numerator = numerator.checked_mul(10)?.checked_add(u128::from(byte - b'0'))?;
        fraction_digits += i32::from(decimal);
    }
    if !digits || numerator == 0 {
        return None;
    }
    numerator = numerator.checked_mul(unit.minutes())?;
    let scale = fraction_digits.checked_sub(exponent)?;
    if scale > 0 {
        let denominator = 10_u128.checked_pow(u32::try_from(scale).ok()?)?;
        numerator.is_multiple_of(denominator).then_some(numerator / denominator)
    } else {
        numerator.checked_mul(10_u128.checked_pow(scale.unsigned_abs())?)
    }
}
