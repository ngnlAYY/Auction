use super::*;

pub(crate) fn normalize_history_query(
    mut query: super::PublicBoardQuery,
) -> super::PublicBoardQuery {
    query.history_from = query.history_from.filter(|value| history_is_valid_date(value));
    query.history_to = query.history_to.filter(|value| history_is_valid_date(value));
    query.history_min_price =
        query.history_min_price.and_then(|value| history_normalized_price(&value));
    query.history_max_price =
        query.history_max_price.and_then(|value| history_normalized_price(&value));
    if query
        .history_from
        .as_deref()
        .zip(query.history_to.as_deref())
        .is_some_and(|(from, to)| from > to)
    {
        query.history_from = None;
        query.history_to = None;
    }
    if query
        .history_min_price
        .as_deref()
        .zip(query.history_max_price.as_deref())
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        query.history_min_price = None;
        query.history_max_price = None;
    }
    query
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
    let Some(value) = value.and_then(history_normalized_price) else {
        return (String::new(), HistoryPriceUnit::K);
    };
    let Ok(amount) = value.parse::<u64>() else {
        return (String::new(), HistoryPriceUnit::K);
    };
    if amount >= 1_000_000 && amount % 10_000 == 0 {
        ((amount / 1_000_000).to_string(), HistoryPriceUnit::M)
    } else {
        ((amount / 1_000).to_string(), HistoryPriceUnit::K)
    }
}

pub(crate) fn history_is_valid_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || !year.chars().all(|value| value.is_ascii_digit()) {
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
    (amount > 0 && amount <= 9_007_199_254_740_991 && amount % 1_000 == 0)
        .then(|| amount.to_string())
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
