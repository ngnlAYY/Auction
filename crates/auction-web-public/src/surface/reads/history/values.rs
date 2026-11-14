//! 价格历史筛选值的规范化与校验。

use super::{HistoryFilterError, HistoryPriceUnit, history_name_value};
use crate::api::PriceHistoryQuery;
use crate::surface::PublicBoardQuery;

pub(crate) fn normalize_history_query(mut query: PublicBoardQuery) -> PublicBoardQuery {
    query.history_name = query.history_name.and_then(|value| match history_name_value(&value) {
        Ok(normalized) => normalized,
        // 非法条件保留给字段错误与请求门禁，不能悄悄丢弃后扩大为无此条件的查询。
        Err(_) => Some(value),
    });
    query.history_from = query.history_from.filter(|value| history_is_valid_date(value));
    query.history_to = query.history_to.filter(|value| history_is_valid_date(value));
    // 非法金额与倒置区间保留为可修正字段；丢弃条件会扩大查询范围。
    query.history_min_price =
        query.history_min_price.map(|value| history_normalized_price(&value).unwrap_or(value));
    query.history_max_price =
        query.history_max_price.map(|value| history_normalized_price(&value).unwrap_or(value));
    if query
        .history_from
        .as_deref()
        .zip(query.history_to.as_deref())
        .is_some_and(|(from, to)| from > to)
    {
        query.history_from = None;
        query.history_to = None;
    }
    query
}

/// URL 名称与金额采用同一字段校验并阻止无效请求；日期沿用既有恢复规则。
pub(crate) fn history_query_error(query: &PriceHistoryQuery) -> Option<HistoryFilterError> {
    if let Err(error) = history_name_value(query.name.as_deref().unwrap_or_default()) {
        return Some(error);
    }
    if query.min_price.as_deref().is_some_and(|value| history_normalized_price(value).is_none()) {
        return Some(HistoryFilterError::MinimumPriceC);
    }
    if query.max_price.as_deref().is_some_and(|value| history_normalized_price(value).is_none()) {
        return Some(HistoryFilterError::MaximumPriceC);
    }
    history_price_range_reversed(query.min_price.as_deref(), query.max_price.as_deref())
        .then_some(HistoryFilterError::PriceRange)
}

pub(crate) fn history_date_parts(value: Option<&str>) -> (String, String) {
    let Some(value) = value.filter(|value| history_is_valid_date(value)) else {
        return (String::new(), String::new());
    };
    let mut parts = value.split('-');
    let year = parts.next().unwrap_or_default().to_owned();
    let month = parts.next().unwrap_or_default().to_owned();
    (month, year)
}

pub(crate) fn history_price_parts(value: Option<&str>) -> (String, HistoryPriceUnit) {
    let Some(raw) = value else {
        return (String::new(), HistoryPriceUnit::K);
    };
    let Some(value) = history_normalized_price(raw) else {
        return (raw.to_owned(), HistoryPriceUnit::C);
    };
    let Ok(amount) = value.parse::<u64>() else {
        return (String::new(), HistoryPriceUnit::K);
    };
    let (unit, scale, digits) = if amount >= 1_000_000 {
        (HistoryPriceUnit::M, 1_000_000, 6)
    } else if amount >= 1_000 {
        (HistoryPriceUnit::K, 1_000, 3)
    } else {
        return (amount.to_string(), HistoryPriceUnit::C);
    };
    let whole = amount / scale;
    let fraction = format!("{:0digits$}", amount % scale);
    let fraction = fraction.trim_end_matches('0');
    let value = if fraction.is_empty() { whole.to_string() } else { format!("{whole}.{fraction}") };
    (value, unit)
}

/// 已完成单值校验的金额区间按整数 Credits 比较；URL 恢复和表单提交必须使用同一规则。
pub(crate) fn history_price_range_reversed(minimum: Option<&str>, maximum: Option<&str>) -> bool {
    minimum
        .and_then(|value| value.parse::<u64>().ok())
        .zip(maximum.and_then(|value| value.parse::<u64>().ok()))
        .is_some_and(|(minimum, maximum)| minimum > maximum)
}

pub(crate) fn history_is_valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || ![year, month, day]
            .into_iter()
            .all(|part| part.chars().all(|value| value.is_ascii_digit()))
    {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) =
        (year.parse::<u32>(), month.parse::<u32>(), day.parse::<u32>())
    else {
        return false;
    };
    year > 0 && (1..=12).contains(&month) && (1..=history_days_in_month(year, month)).contains(&day)
}

pub(crate) fn history_normalized_price(value: &str) -> Option<String> {
    let amount = value.parse::<u64>().ok()?;
    (amount <= 9_007_199_254_740_991).then(|| amount.to_string())
}

pub(crate) fn history_days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(crate) fn history_active_filter_count(query: &PriceHistoryQuery) -> usize {
    [
        query.world.as_deref().filter(|value| *value != "all"),
        query.name.as_deref(),
        query.from.as_deref(),
        query.to.as_deref(),
        query.min_price.as_deref(),
        query.max_price.as_deref(),
        query.buyer.as_deref(),
        query.seller.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|value| !value.is_empty())
    .count()
}

pub(crate) fn optional_filter_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
