//! 价格历史筛选值的规范化与校验。

use auction_time::{Duration, Timestamp, parse_date};

use super::filters::model::{HistoryErrors, HistoryField, HistoryFilterError, HistoryPriceUnit};
use super::filters::validation::history_name_value;
use crate::api::PriceHistoryQuery;
use crate::surface::{PublicBoardQuery, public_page_within_offset, query_values};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct HistoryAddress {
    pub(super) query: PriceHistoryQuery,
    pub(super) errors: HistoryErrors,
    raw: [Vec<String>; 8],
}

impl HistoryAddress {
    pub(super) fn read(query: &PublicBoardQuery, pairs: &[(String, String)]) -> Self {
        // 普通地址解析会省略部分非法值。这里保留各键原文，让请求门禁与表单查看同一份输入。
        let raw = HistoryField::ALL
            .map(|field| query_values(pairs, field.key()).map(str::to_owned).collect::<Vec<_>>());
        let get = |field: HistoryField| {
            raw[field as usize].first().filter(|value| !value.is_empty()).cloned()
        };
        let name = get(HistoryField::Name);
        let query = PriceHistoryQuery {
            world: get(HistoryField::World).filter(|world| world != "all"),
            name: match name.as_deref().map(history_name_value) {
                Some(Ok(value)) => value,
                _ => name,
            },
            from: get(HistoryField::From),
            to: get(HistoryField::To),
            min_price: get(HistoryField::Minimum),
            max_price: get(HistoryField::Maximum),
            buyer: get(HistoryField::Buyer),
            seller: get(HistoryField::Seller),
            sort: query.history_sort.clone(),
            page: query.page,
        };
        let mut errors = HistoryErrors::default();
        for field in HistoryField::ALL {
            if raw[field as usize].len() > 1 {
                errors.set(field, Some(HistoryFilterError::Duplicate));
            }
        }
        // 名称字节限制必须按拆词和去空白前的原文判断。
        history_query_errors(&query, &mut errors);
        Self { query, errors, raw }
    }

    pub(super) fn raw_values(&self, field: HistoryField) -> &[String] {
        &self.raw[field as usize]
    }

    pub(super) fn form(mut self) -> Self {
        self.query.sort.clear();
        self.query.page = 1;
        self
    }

    pub(super) fn has_request(&self) -> bool {
        self.errors.first().is_none() && history_active_filter_count(&self.query) != 0
    }

    pub(super) fn request(&self, page_size: i64) -> Option<(PriceHistoryQuery, i64)> {
        // 两类非法地址都应暂停 GET：筛选待修正，或页码尚未成功替换。
        (self.has_request() && public_page_within_offset(self.query.page, page_size))
            .then(|| (self.query.clone(), page_size))
    }
}

/// 提交与恢复链接共用校验；字节数与控制字符按输入原文判定，玩家全名不裁去空白。
pub(super) fn history_query_errors(query: &PriceHistoryQuery, errors: &mut HistoryErrors) {
    for field in HistoryField::ALL {
        if errors.get(field).is_some() {
            continue;
        }
        let Some(value) = field.value(query) else { continue };
        let error = match field {
            HistoryField::World => {
                (!matches!(value, "main" | "isekai")).then_some(HistoryFilterError::WorldInvalid)
            }
            HistoryField::Name => history_name_value(value).err(),
            HistoryField::From => history_date_boundary(value, false)
                .is_none()
                .then_some(HistoryFilterError::FromDateInvalid),
            HistoryField::To => history_date_boundary(value, true)
                .is_none()
                .then_some(HistoryFilterError::ToDateInvalid),
            HistoryField::Minimum => history_normalized_price(value)
                .is_none()
                .then_some(HistoryFilterError::MinimumPriceC),
            HistoryField::Maximum => history_normalized_price(value)
                .is_none()
                .then_some(HistoryFilterError::MaximumPriceC),
            HistoryField::Buyer | HistoryField::Seller => history_player_error(value),
        };
        errors.set(field, error);
    }
    if errors.get(HistoryField::From).is_none()
        && errors.get(HistoryField::To).is_none()
        && query.from.as_deref().zip(query.to.as_deref()).is_some_and(|(from, to)| from > to)
    {
        errors.set(HistoryField::From, Some(HistoryFilterError::DateRange));
    }
    if errors.get(HistoryField::Minimum).is_none()
        && errors.get(HistoryField::Maximum).is_none()
        && history_price_range_reversed(query.min_price.as_deref(), query.max_price.as_deref())
    {
        errors.set(HistoryField::Minimum, Some(HistoryFilterError::PriceRange));
    }
}

fn history_date_boundary(value: &str, end: bool) -> Option<Timestamp> {
    if !history_is_valid_date(value) {
        return None;
    }
    let date = parse_date(value)?;
    let midnight = Timestamp::from_utc_datetime(date.at(0, 0, 0, 0))?;
    let boundary = if end { midnight.checked_add_signed(Duration::days(1))? } else { midnight };
    (boundary.timestamp_micros() != 0).then_some(boundary)
}

pub(super) fn history_player_error(value: &str) -> Option<HistoryFilterError> {
    if value.len() > 256 {
        Some(HistoryFilterError::PlayerLength)
    } else {
        value.chars().any(char::is_control).then_some(HistoryFilterError::PlayerInvalid)
    }
}

pub(crate) fn history_date_parts(value: Option<&str>) -> (String, String) {
    let Some(value) = value else {
        return (String::new(), String::new());
    };
    if !history_is_valid_date(value) {
        return (String::new(), value.to_owned());
    }
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
    if !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
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

#[cfg(test)]
mod tests {
    use super::super::filters::model::{HistoryField, HistoryFilterError};
    use super::HistoryAddress;
    use crate::surface::PublicBoardQuery;

    fn restore(pairs: &[(&str, &str)]) -> HistoryAddress {
        HistoryAddress::read(
            &PublicBoardQuery::default(),
            &pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn invalid_history_players_block_an_otherwise_valid_search() {
        let over_limit = "買".repeat(86);
        let invalid = restore(&[("name", "Robe"), ("buyer", &over_limit), ("seller", "Seller\n")]);
        assert_eq!(invalid.query.buyer.as_deref(), Some(over_limit.as_str()));
        assert_eq!(invalid.query.seller.as_deref(), Some("Seller\n"));
        assert_eq!(invalid.errors.get(HistoryField::Buyer), Some(HistoryFilterError::PlayerLength));
        assert_eq!(
            invalid.errors.get(HistoryField::Seller),
            Some(HistoryFilterError::PlayerInvalid)
        );
        assert!(invalid.request(20).is_none());
        let valid_name = format!("{}x", "買".repeat(85));
        let corrected =
            restore(&[("name", "Robe"), ("buyer", &valid_name), ("seller", " Seller ")]);
        assert_eq!(corrected.query.buyer.as_deref(), Some(valid_name.as_str()));
        assert_eq!(corrected.query.seller.as_deref(), Some(" Seller "));
        assert_eq!(corrected.request(20), Some((corrected.query, 20)));
    }

    #[test]
    fn reversed_history_dates_keep_both_endpoints_for_correction() {
        let raw = restore(&[("from", "2026-04-01"), ("to", "2026-03-31")]);
        assert_eq!(raw.query.from.as_deref(), Some("2026-04-01"));
        assert_eq!(raw.query.to.as_deref(), Some("2026-03-31"));
        assert_eq!(raw.errors.get(HistoryField::From), Some(HistoryFilterError::DateRange));
        assert!(!raw.has_request());
        assert!(restore(&[("from", "2026-04-01"), ("to", "2026-04-01")]).has_request());
        for (key, date) in [
            ("from", "1970-01-01"),
            ("to", "1969-12-31"),
            ("from", "9999-12-31"),
            ("to", "9999-12-30"),
            ("from", "2025-02-29"),
            ("to", "2026/01/01"),
        ] {
            let invalid = restore(&[(key, date), ("name", "Robe")]);
            assert!(!invalid.has_request(), "{key}={date}");
        }
    }

    #[test]
    fn duplicates_and_unknown_world_cannot_become_an_unfiltered_search() {
        let raw =
            restore(&[("world", "unknown"), ("buyer", ""), ("buyer", "One"), ("name", "Robe")]);
        assert_eq!(raw.errors.get(HistoryField::World), Some(HistoryFilterError::WorldInvalid));
        assert_eq!(raw.errors.get(HistoryField::Buyer), Some(HistoryFilterError::Duplicate));
        assert_eq!(raw.raw_values(HistoryField::Buyer), ["", "One"]);
        assert!(!raw.has_request());
        assert!(restore(&[("world", "all"), ("buyer", "One"), ("name", "Robe")]).has_request());
        assert!(!restore(&[("name", "Robe"), ("min_price", "000")]).has_request());
    }
}
