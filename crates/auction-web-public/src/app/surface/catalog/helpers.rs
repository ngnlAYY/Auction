//! Public 拍卖目录的分类、状态、链接、金额和倒计时映射。

use super::*;
use auction_contracts::MAX_PUBLIC_BID_AMOUNT;

/// 按分类把目录行分组。返回行下标而非克隆行：调用方持有的行向量全程以 Rc 共享，
/// 分组不再产生第二层逐字段克隆。
pub(crate) fn grouped_catalog_rows(
    rows: &[CatalogRow],
    active: &str,
) -> Vec<(&'static str, Vec<usize>)> {
    let mut groups: Vec<(&str, Vec<usize>)> =
        CATALOG_CATEGORIES.iter().map(|(category, _)| (*category, Vec::new())).collect();
    let mut other = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let category = catalog_category_key(&row.category);
        if active != "all" && active != category {
            continue;
        }
        if let Some((_, entries)) = groups.iter_mut().find(|(key, _)| *key == category) {
            entries.push(index);
        } else if active == "all" || active == "other" {
            other.push(index);
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

/// 稀有与融合独立于装备品质和类型；此处仅翻译服务端给出的分组。
pub(crate) fn equipment_group_label(equipment_group: &str) -> String {
    match equipment_group {
        "rare" => t!("equipment-group-rare"),
        "fusion" => t!("equipment-group-fusion"),
        _ => t!("value-empty"),
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
        "settled" => t!("auction-status-settled"),
        "archived" => t!("auction-status-archived"),
        "cancelled" => t!("auction-status-cancelled"),
        "sold" => t!("status-ended"),
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

pub(crate) fn historical_auction_status_label(status: &str) -> String {
    match status {
        "live" => t!("auction-history-status-live"),
        "final" => t!("auction-history-status-final"),
        "settled" => t!("auction-history-status-settled"),
        _ => auction_status_label(status),
    }
}

pub(crate) fn auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

pub(crate) fn historical_lots_href(auction_id: &str, world_id: &str) -> String {
    let query = PublicBoardQuery {
        auction_id: Some(auction_id.to_owned()),
        world: (world_id != "main").then(|| world_id.to_owned()),
        ..PublicBoardQuery::default()
    };
    public_href("/", &query)
}

pub(crate) fn historical_bid_log_href(auction_id: &str, world_id: &str) -> String {
    let query = PublicBoardQuery {
        auction_id: Some(auction_id.to_owned()),
        world: (world_id != "main").then(|| world_id.to_owned()),
        ..PublicBoardQuery::default()
    };
    public_href("/bidlog", &query)
}

pub(crate) fn lot_detail_href(lot_code: &str) -> String {
    public_href(&format!("/lots/{lot_code}"), &read_public_query())
}

/// 已知世界的入口不能借用地址栏的默认世界；裸详情仍会按拍卖 UUID 复核。
pub(crate) fn lot_detail_world_href(lot_id: &str, world: &str) -> String {
    let query =
        PublicBoardQuery { world: normalize_world_query(world), ..PublicBoardQuery::default() };
    public_href(&format!("/lots/{lot_id}"), &query)
}

/// 账户竞价 wire 只有拍卖 UUID，没有世界；保留已知身份，不推测世界。
pub(crate) fn lot_detail_auction_href(lot_id: &str, auction_id: &str) -> String {
    let query =
        PublicBoardQuery { auction_id: Some(auction_id.to_owned()), ..PublicBoardQuery::default() };
    public_href(&format!("/lots/{lot_id}"), &query)
}

pub(crate) fn current_auction_href() -> String {
    public_href("/auction", &read_public_query())
}

pub(crate) fn format_auction_time(value: &str) -> String {
    format_localized_time_for_language(value, &i18n().language().to_string())
}

/// 解析 credit 缩写；无后缀或 K 使用整数千，M/B 最多支持三/六位小数，精确到信用网格。
/// 浏览器只用该结果做即时提示，服务端仍会再次校验金额、拍卖状态和竞价资格。
pub(crate) fn parse_credit_amount(input: &str) -> Option<i64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (number, scale, maximum_decimals) = match input.chars().last() {
        Some('k' | 'K') => (&input[..input.len() - 1], 1_000_i64, 0),
        Some('m' | 'M') => (&input[..input.len() - 1], 1_000_000_i64, 3),
        Some('b' | 'B') => (&input[..input.len() - 1], 1_000_000_000_i64, 6),
        Some(_) => (input, 1_000_i64, 0),
        None => return None,
    };
    let (whole, fraction) =
        number.split_once('.').map_or((number, None), |(whole, fraction)| (whole, Some(fraction)));
    if whole.is_empty() || !whole.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    if fraction.is_some_and(|value| {
        value.is_empty()
            || value.len() > maximum_decimals
            || !value.chars().all(|character| character.is_ascii_digit())
    }) {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?;
    let fractional_credits = match fraction {
        Some(fraction) => {
            let divisor = 10_i64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
            fraction.parse::<i64>().ok()?.checked_mul(scale / divisor)?
        }
        None => 0,
    };
    let amount = whole.checked_mul(scale)?.checked_add(fractional_credits)?;
    (amount > 0 && amount <= MAX_PUBLIC_BID_AMOUNT).then_some(amount)
}

pub(crate) struct CountdownDisplay {
    pub(crate) label: String,
    pub(crate) urgent: bool,
    pub(crate) expired: bool,
    pub(crate) empty: bool,
}

/// 截止文本变化时才重新解析；秒针和语言变化仍即时更新显示，不缓存服务端校时结果。
pub(crate) fn use_countdown_display(ends_at: String) -> CountdownDisplay {
    let end = use_memo(use_reactive((&ends_at,), |(ends_at,)| {
        DateTime::parse_from_rfc3339(&ends_at).ok().map(|value| value.with_timezone(&Utc))
    }));
    let tick = use_second_tick();
    let _ = tick();
    countdown_display(end(), estimated_now())
}

fn countdown_display(end: Option<DateTime<Utc>>, now: DateTime<Utc>) -> CountdownDisplay {
    let Some(end) = end else {
        return CountdownDisplay {
            label: t!("value-empty"),
            urgent: false,
            expired: false,
            empty: true,
        };
    };
    let remaining_seconds = (end - now).num_seconds();
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
