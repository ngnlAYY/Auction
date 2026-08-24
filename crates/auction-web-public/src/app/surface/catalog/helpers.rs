//! Public 拍卖目录的分类、状态、链接、金额和倒计时映射。

use super::*;

pub(crate) fn grouped_catalog_rows(
    rows: &[CatalogRow],
    active: &str,
) -> Vec<(&'static str, Vec<CatalogRow>)> {
    let mut groups: Vec<(&str, Vec<CatalogRow>)> =
        CATALOG_CATEGORIES.iter().map(|(category, _)| (*category, Vec::new())).collect();
    let mut other = Vec::new();

    for row in rows {
        let category = catalog_category_key(&row.category);
        if active != "all" && active != category {
            continue;
        }
        if let Some((_, entries)) = groups.iter_mut().find(|(key, _)| *key == category) {
            entries.push(row.clone());
        } else if active == "all" || active == "other" {
            other.push(row.clone());
        }
    }

    let mut result =
        groups.into_iter().filter(|(_, entries)| !entries.is_empty()).collect::<Vec<_>>();
    if !other.is_empty() {
        result.push(("other", other));
    }
    result
}

pub(crate) fn catalog_count(
    counts: &BTreeMap<String, i64>,
    category: &str,
    rows: &[CatalogRow],
) -> i64 {
    counts.get(category).copied().unwrap_or_else(|| {
        if category == "all" {
            rows.len() as i64
        } else {
            rows.iter().filter(|row| catalog_category_key(&row.category) == category).count() as i64
        }
    })
}

pub(crate) fn catalog_category_key(category: &str) -> &'static str {
    match category {
        "one" | "One-Handed" | "one-handed" => "one",
        "two" | "Two-Handed" | "two-handed" => "two",
        "sta" | "Staff" | "staff" => "sta",
        "shd" | "Shield" | "shield" => "shd",
        "clo" | "Cloth" | "cloth" => "clo",
        "lig" | "Light" | "light" => "lig",
        "hea" | "Heavy" | "heavy" => "hea",
        _ => "other",
    }
}

pub(crate) fn catalog_category_code(category: &str) -> &'static str {
    CATALOG_CATEGORIES
        .iter()
        .find_map(|(key, code)| (*key == category).then_some(*code))
        .unwrap_or("Other")
}

pub(crate) fn catalog_category_label(category: &str) -> String {
    match category {
        "one" => t!("category-one-handed"),
        "two" => t!("category-two-handed"),
        "sta" => t!("category-staff"),
        "shd" => t!("category-shield"),
        "clo" => t!("category-cloth"),
        "lig" => t!("category-light"),
        "hea" => t!("category-heavy"),
        _ => t!("category-other"),
    }
}

pub(crate) fn lot_status_label(status: &str) -> String {
    match status {
        "pending" => t!("status-pending"),
        "live" => t!("status-live"),
        "sold" => t!("status-sold"),
        "unsold" => t!("status-unsold"),
        _ => status.to_owned(),
    }
}

pub(crate) fn lot_status_class(status: &str) -> &'static str {
    match status {
        "pending" => "pending",
        "live" => "live",
        "sold" => "sold",
        "unsold" => "unsold",
        _ => "unknown",
    }
}

pub(crate) fn auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("auction-status-draft"),
        "live" => t!("auction-status-live"),
        "preview" => t!("auction-status-preview"),
        "final" => t!("auction-status-final"),
        "settling" => t!("auction-status-settling"),
        "cancelled" => t!("auction-status-cancelled"),
        "sold" | "settled" => t!("status-ended"),
        _ => status.to_owned(),
    }
}

pub(crate) fn auction_status_class(status: &str) -> &'static str {
    match status {
        "draft" => "draft",
        "live" => "live",
        "preview" => "preview",
        "final" => "final",
        "settling" => "settling",
        "cancelled" => "cancelled",
        "sold" => "sold",
        "settled" => "settled",
        _ => "unknown",
    }
}

pub(crate) fn auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

pub(crate) fn historical_auction_href(auction_id: &str, world_id: Option<&str>) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = world_id.and_then(normalize_world_query);
    public_href("/auction", &query)
}

pub(crate) fn historical_history_href(auction_id: &str, world_id: &str) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = normalize_world_query(world_id);
    query.category = None;
    query.status = None;
    query.page = 1;
    public_href("/auction", &query)
}

pub(crate) fn lot_detail_href(lot_code: &str) -> String {
    public_href(&format!("/lots/{lot_code}"), &read_public_query())
}

pub(crate) fn current_auction_href() -> String {
    public_href("/auction", &read_public_query())
}

pub(crate) fn format_auction_time(value: &str) -> String {
    format_localized_time(value)
}

/// 解析旧版 Public 使用的 credit 缩写；无后缀和 `k` 表示千，`m` 支持最多两位小数。
///
/// 浏览器只用该结果做即时提示，服务端仍会再次校验金额、拍卖状态和竞价资格。
pub(crate) fn parse_credit_amount(input: &str) -> Option<i64> {
    const MAX_SAFE_JSON_INTEGER: i64 = 9_007_199_254_740_991;

    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (number, suffix) = match input.chars().last() {
        Some(character) if matches!(character, 'k' | 'K' | 'm' | 'M') => {
            (&input[..input.len() - character.len_utf8()], Some(character))
        }
        Some(_) => (input, None),
        None => return None,
    };
    let (whole, fraction) =
        number.split_once('.').map_or((number, None), |(whole, fraction)| (whole, Some(fraction)));
    if whole.is_empty() || !whole.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    if fraction.is_some_and(|value| {
        value.is_empty()
            || value.len() > 2
            || !value.chars().all(|character| character.is_ascii_digit())
    }) {
        return None;
    }
    if suffix.is_some_and(|character| matches!(character, 'k' | 'K')) && fraction.is_some() {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?;
    let amount = match suffix {
        Some('m' | 'M') => {
            let base = whole.checked_mul(1_000_000)?;
            let fraction = fraction
                .and_then(|value| {
                    let scale = if value.len() == 1 { 100_000 } else { 10_000 };
                    value.parse::<i64>().ok()?.checked_mul(scale)
                })
                .unwrap_or(0);
            base.checked_add(fraction)?
        }
        Some('k' | 'K') | None => whole.checked_mul(1_000)?,
        Some(_) => return None,
    };
    (amount > 0 && amount <= MAX_SAFE_JSON_INTEGER).then_some(amount)
}

pub(crate) struct CountdownDisplay {
    pub(crate) label: String,
    pub(crate) urgent: bool,
    pub(crate) expired: bool,
    pub(crate) empty: bool,
}

pub(crate) fn countdown_display(ends_at: &str, now: DateTime<Utc>) -> CountdownDisplay {
    let Some(end) = DateTime::parse_from_rfc3339(ends_at).ok() else {
        return CountdownDisplay {
            label: t!("value-empty"),
            urgent: false,
            expired: false,
            empty: true,
        };
    };
    let remaining_seconds = (end.with_timezone(&Utc) - now).num_seconds();
    if remaining_seconds <= 0 {
        return CountdownDisplay {
            label: t!("auction-countdown-expired"),
            urgent: false,
            expired: true,
            empty: false,
        };
    }
    let days = remaining_seconds / 86_400;
    let hours = (remaining_seconds % 86_400) / 3_600;
    let minutes = (remaining_seconds % 3_600) / 60;
    let seconds = remaining_seconds % 60;
    let clock = format!("{hours:02}:{minutes:02}:{seconds:02}");
    CountdownDisplay {
        label: if days > 0 {
            t!("auction-countdown-days", days: days, clock: clock)
        } else {
            clock
        },
        urgent: remaining_seconds <= 15 * 60,
        expired: false,
        empty: false,
    }
}

pub(crate) fn countdown_class(countdown: &CountdownDisplay) -> &'static str {
    if countdown.empty {
        "public-auction-countdown public-auction-countdown--none"
    } else if countdown.urgent {
        "public-auction-countdown public-auction-countdown--closing"
    } else if countdown.expired {
        "public-auction-countdown public-auction-countdown--expired"
    } else {
        "public-auction-countdown"
    }
}

/// Public 数据加载、错误和空结果共用的状态面板。
///
/// 该组件只呈现调用方已经判定的安全文案，不吞掉 API 错误，也不把错误正文直接展示给
/// 浏览器；真实数据页面仍由各自的 API 读取分支负责。
#[component]
pub(crate) fn Placeholder(title: String, detail: String) -> Element {
    rsx! {
        section {
            class: "public-auction-empty",
            role: "status",
            aria_live: "polite",
            aria_labelledby: "placeholder-title",
            span { class: "public-auction-empty__mark", aria_hidden: "true" }
            div { class: "public-auction-empty__copy",
                h1 { id: "placeholder-title", "{title}" }
                p { "{detail}" }
            }
        }
    }
}
