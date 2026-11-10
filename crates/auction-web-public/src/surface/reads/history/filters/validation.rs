//! 价格历史筛选器的错误文案、日期边界和金额规范化。

use super::super::{history_date_parts, history_days_in_month, history_is_valid_date};
use super::{HistoryDateError, HistoryFilterError, HistoryPriceError, HistoryPriceUnit};
use auction_web_shared::legacy_t as t;

/// 名称字段复用 HTTP 的空白/逗号拆词和应用端口字节边界；缩写片段均须命中装备名。
pub(crate) fn history_name_value(value: &str) -> Result<Option<String>, HistoryFilterError> {
    let value = value.trim();
    if value.len() > 1_024 {
        return Err(HistoryFilterError::NameTooLong);
    }
    let mut term_count = 0;
    let mut term_too_long = false;
    let mut invalid = false;
    for term in value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|term| !term.is_empty())
    {
        term_count += 1;
        if term_count > 8 {
            return Err(HistoryFilterError::NameTooManyTerms);
        }
        term_too_long |= term.len() > 128;
        invalid = invalid || (!term_too_long && term.chars().any(char::is_control));
    }
    // 保留总长、词数、单词长度、控制字符的既有错误优先级，不为校验建立词数组。
    if term_too_long {
        return Err(HistoryFilterError::NameTermTooLong);
    }
    if invalid {
        return Err(HistoryFilterError::NameInvalid);
    }
    // 只有分隔符的输入与服务端的空词集合一致，不把它当成有效条件加载全部历史。
    Ok((term_count != 0).then(|| value.to_owned()))
}

/// 将名称字段的稳定错误分类转换为本 surface 的文案。
pub(crate) fn history_name_error(error: Option<HistoryFilterError>) -> Option<String> {
    match error? {
        HistoryFilterError::NameTooManyTerms => Some(t!("history-filter-error-name-terms")),
        HistoryFilterError::NameTermTooLong => Some(t!("history-filter-error-name-term-length")),
        HistoryFilterError::NameTooLong => Some(t!("history-filter-error-name-length")),
        HistoryFilterError::NameInvalid => Some(t!("history-filter-error-name-invalid")),
        _ => None,
    }
}

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
        (HistoryFilterError::MinimumPriceC, true) | (HistoryFilterError::MaximumPriceC, false) => {
            Some(t!("history-filter-error-price-c"))
        }
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

/// 年月未改变时保留当前 URL 的精确日期；改变年月才生成整月的闭区间端点。
pub(crate) fn history_date_value(
    month: &str,
    year: &str,
    end: bool,
    applied: Option<&str>,
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
    if let Some(applied) = applied.filter(|value| history_is_valid_date(value)) {
        let (applied_month, applied_year) = history_date_parts(Some(applied));
        if applied_month.parse::<u32>().ok() == Some(month_number)
            && applied_year.parse::<u32>().ok() == Some(year_number)
        {
            return Ok(Some(applied.to_owned()));
        }
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
    let precision = match unit {
        HistoryPriceUnit::C => 0,
        HistoryPriceUnit::K => 3,
        HistoryPriceUnit::M => 6,
    };
    let (whole, fraction) = match unit {
        HistoryPriceUnit::C => {
            if !value.chars().all(|character| character.is_ascii_digit()) {
                return Err(HistoryPriceError::Invalid);
            }
            (value, "")
        }
        HistoryPriceUnit::K | HistoryPriceUnit::M => {
            let mut parts = value.split('.');
            let whole = parts.next().unwrap_or_default();
            let fraction = parts.next().unwrap_or_default();
            if parts.next().is_some()
                || whole.is_empty()
                || !whole.chars().all(|character| character.is_ascii_digit())
                || (fraction.is_empty() && value.contains('.'))
                || fraction.len() > precision
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
    // 仅对整数补足单位小数位；全程 checked 整数运算，不能用浮点或截断非整千 Credits。
    let fraction_digits = fraction.len() as u32;
    let fraction = fraction.parse::<u64>().unwrap_or_default();
    let credits = whole.checked_mul(10u64.pow(precision as u32)).and_then(|credits| {
        fraction
            .checked_mul(10u64.pow(precision as u32 - fraction_digits))
            .and_then(|fraction| credits.checked_add(fraction))
    });
    let Some(credits) = credits else {
        return Err(HistoryPriceError::Invalid);
    };
    if credits > 9_007_199_254_740_991 {
        return Err(HistoryPriceError::Invalid);
    }
    Ok(Some(credits.to_string()))
}
