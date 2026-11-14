//! 价格历史筛选器的错误文案、日期边界和金额规范化。

use super::super::{history_date_parts, history_days_in_month, history_is_valid_date};
use super::{HistoryDateError, HistoryFilterError, HistoryPriceError, HistoryPriceUnit};
use auction_web_shared::t;

/// 名称查询的词数上限，与 HTTP 层的 `MAX_PUBLIC_HISTORY_NAME_TERMS` 一致。
const MAX_NAME_TERMS: usize = 8;

fn name_terms(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|term| !term.is_empty())
}

/// 名称字段复用 HTTP 的空白/逗号拆词和应用端口字节边界；缩写片段均须命中装备名。
pub(crate) fn history_name_value(value: &str) -> Result<Option<String>, HistoryFilterError> {
    let value = value.trim();
    if value.len() > 1_024 {
        return Err(HistoryFilterError::NameTooLong);
    }
    let mut term_count = 0;
    let mut term_too_long = false;
    let mut invalid = false;
    for term in name_terms(value) {
        term_count += 1;
        if term_count > MAX_NAME_TERMS {
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

/// 把完整装备名转成价格历史的名称查询。服务端只做“各词都包含”的匹配，没有精确等于，
/// 因此结果可能包含名称更长的同类装备，调用方需自行标出不同名的行。
/// 超出词数上限时保留最长的词（最具区分度）并维持原顺序；没有可用词条或名称
/// 不满足查询约束时返回 `None`，调用方不发请求。
pub(crate) fn history_equipment_name_query(equipment_name: &str) -> Option<String> {
    let terms: Vec<&str> = name_terms(equipment_name).collect();
    let query = if terms.len() <= MAX_NAME_TERMS {
        terms.join(" ")
    } else {
        let mut longest: Vec<usize> = (0..terms.len()).collect();
        // 稳定排序让等长的词保留靠前者，同一名称始终得到同一查询。
        longest.sort_by_key(|&index| std::cmp::Reverse(terms[index].len()));
        longest.truncate(MAX_NAME_TERMS);
        longest.sort_unstable();
        longest.into_iter().map(|index| terms[index]).collect::<Vec<_>>().join(" ")
    };
    history_name_value(&query).ok().flatten()
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

#[cfg(test)]
mod tests {
    use super::history_equipment_name_query;

    #[test]
    fn equipment_name_within_the_term_limit_is_queried_as_is() {
        assert_eq!(
            history_equipment_name_query("  Legendary Charged Cotton Gloves of the Fire-eater "),
            Some("Legendary Charged Cotton Gloves of the Fire-eater".to_owned())
        );
    }

    #[test]
    fn equipment_name_over_the_term_limit_keeps_the_longest_terms_in_order() {
        assert_eq!(
            history_equipment_name_query(
                "Legendary Ethereal Mithril Power Helmet of the Heaven-sent Slaughter"
            ),
            Some("Legendary Ethereal Mithril Power Helmet the Heaven-sent Slaughter".to_owned())
        );
        // 等长的词保留靠前者：九个等长词丢弃最后一个。
        assert_eq!(
            history_equipment_name_query("aa bb cc dd ee ff gg hh ii"),
            Some("aa bb cc dd ee ff gg hh".to_owned())
        );
    }

    #[test]
    fn equipment_name_without_usable_terms_yields_no_query() {
        assert_eq!(history_equipment_name_query(""), None);
        assert_eq!(history_equipment_name_query(" , \t"), None);
        assert_eq!(history_equipment_name_query("Legendary\u{7}Axe"), None);
        assert_eq!(history_equipment_name_query(&"x".repeat(129)), None);
    }
}
