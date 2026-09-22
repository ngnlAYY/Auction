//! 价格历史筛选器的错误文案、日期边界和金额规范化。

use super::super::history_days_in_month;
use super::{HistoryDateError, HistoryFilterError, HistoryPriceError, HistoryPriceUnit};
use auction_web_shared::legacy_t as t;

/// 将页面错误分类转换为对应的本地化日期错误。
pub(crate) fn history_date_error(error: Option<HistoryFilterError>, from: bool) -> Option<String> {
    match (error?, from) {
        (HistoryFilterError::FromDateIncomplete, true)
        | (HistoryFilterError::ToDateIncomplete, false) => {
            Some(t!("history-filter-error-date-incomplete"))
        }
        (HistoryFilterError::FromDateInvalid, true)
        | (HistoryFilterError::ToDateInvalid, false) => {
            Some(t!("history-filter-error-date-invalid"))
        }
        (HistoryFilterError::DateRange, true) => Some(t!("history-filter-error-date-range")),
        _ => None,
    }
}

/// 将页面错误分类转换为对应的本地化金额错误。
pub(crate) fn history_price_error(
    error: Option<HistoryFilterError>,
    minimum: bool,
) -> Option<String> {
    match (error?, minimum) {
        (HistoryFilterError::MinimumPriceK, true) | (HistoryFilterError::MaximumPriceK, false) => {
            Some(t!("history-filter-error-price-k"))
        }
        (HistoryFilterError::MinimumPriceM, true) | (HistoryFilterError::MaximumPriceM, false) => {
            Some(t!("history-filter-error-price-m"))
        }
        (HistoryFilterError::PriceRange, true) => Some(t!("history-filter-error-price-range")),
        _ => None,
    }
}

/// 返回筛选器使用的本地化月份名称。
pub(crate) fn history_month_label(month: i32) -> String {
    match month {
        1 => t!("history-filter-month-01"),
        2 => t!("history-filter-month-02"),
        3 => t!("history-filter-month-03"),
        4 => t!("history-filter-month-04"),
        5 => t!("history-filter-month-05"),
        6 => t!("history-filter-month-06"),
        7 => t!("history-filter-month-07"),
        8 => t!("history-filter-month-08"),
        9 => t!("history-filter-month-09"),
        10 => t!("history-filter-month-10"),
        11 => t!("history-filter-month-11"),
        12 => t!("history-filter-month-12"),
        _ => month.to_string(),
    }
}

/// 将年月输入转换为 API 使用的闭区间日期端点。
pub(crate) fn history_date_value(
    month: &str,
    year: &str,
    end: bool,
) -> Result<Option<String>, HistoryDateError> {
    let month = month.trim();
    let year = year.trim();
    if month.is_empty() && year.is_empty() {
        return Ok(None);
    }
    if month.is_empty() || year.is_empty() {
        return Err(HistoryDateError::Incomplete);
    }
    if year.len() != 4 || !year.chars().all(|value| value.is_ascii_digit()) {
        return Err(HistoryDateError::Invalid);
    }
    let Ok(year_number) = year.parse::<u32>() else {
        return Err(HistoryDateError::Invalid);
    };
    let Ok(month_number) = month.parse::<u32>() else {
        return Err(HistoryDateError::Invalid);
    };
    if year_number == 0 || !(1..=12).contains(&month_number) {
        return Err(HistoryDateError::Invalid);
    }
    let day = if end { history_days_in_month(year_number, month_number) } else { 1 };
    Ok(Some(format!("{year_number:04}-{month_number:02}-{day:02}")))
}

/// 将带单位的页面金额输入规范化为 API 使用的整数 Credits。
pub(crate) fn history_price_value(
    value: &str,
    unit: HistoryPriceUnit,
) -> Result<Option<String>, HistoryPriceError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let (whole, fraction) = match unit {
        HistoryPriceUnit::K => {
            if !value.chars().all(|character| character.is_ascii_digit()) {
                return Err(HistoryPriceError::Invalid);
            }
            (value, "")
        }
        HistoryPriceUnit::M => {
            let mut parts = value.split('.');
            let whole = parts.next().unwrap_or_default();
            let fraction = parts.next().unwrap_or_default();
            if parts.next().is_some()
                || whole.is_empty()
                || !whole.chars().all(|character| character.is_ascii_digit())
                || (fraction.is_empty() && value.contains('.'))
                || fraction.len() > 2
                || !fraction.chars().all(|character| character.is_ascii_digit())
            {
                return Err(HistoryPriceError::Invalid);
            }
            (whole, fraction)
        }
    };
    let Ok(whole) = whole.parse::<u64>() else {
        return Err(HistoryPriceError::Invalid);
    };
    let credits = match unit {
        HistoryPriceUnit::K => whole.checked_mul(1_000),
        HistoryPriceUnit::M => {
            let fraction_digits = fraction.len() as u32;
            let fraction = fraction.parse::<u64>().unwrap_or_default();
            whole
                .checked_mul(1_000_000)
                .and_then(|credits| credits.checked_add(fraction * 10u64.pow(6 - fraction_digits)))
        }
    };
    let Some(credits) = credits else {
        return Err(HistoryPriceError::Invalid);
    };
    if credits == 0 || credits > 9_007_199_254_740_991 {
        return Err(HistoryPriceError::Invalid);
    }
    Ok(Some(credits.to_string()))
}
