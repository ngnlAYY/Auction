//! 地址栏 query 的解析：按键分派、值规范化与非法值处理；未声明的键原样透传。
//! 只在浏览器与测试构建中编译，宿主构建的查询状态恒为默认值。

use url::form_urlencoded;
use uuid::Uuid;

use super::{CATALOG_CATEGORIES, PublicBoardQuery, normalize_world_query};
use crate::surface::{DEFAULT_PAGE_SIZE, parse_public_page_size};

pub(super) fn parse_public_query(pathname: &str, search: &str) -> PublicBoardQuery {
    let raw = search.strip_prefix('?').unwrap_or_default();
    let mut query = PublicBoardQuery::default();
    let mut canonical_page = None;
    let mut legacy_bid_log_page = None;
    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "world" => query.world = normalize_world_query(&value),
            "auction" => query.auction_id = Uuid::parse_str(&value).ok().map(|id| id.to_string()),
            "type" if is_catalog_category(&value) => query.category = Some(value.into_owned()),
            "equipment_group" => {
                query.equipment_group =
                    matches!(value.as_ref(), "rare" | "fusion").then(|| value.into_owned());
            }
            "max_level" => query.max_level = Some(value.into_owned()),
            "include_unleveled" => query.include_unleveled = value.into_owned(),
            "status" if is_history_status(&value) => query.status = Some(value.into_owned()),
            "name" => query.history_name = retained_query_value(&value),
            "from" => query.history_from = normalize_optional_query(&value, 10),
            "to" => query.history_to = normalize_optional_query(&value, 10),
            "min_price" => {
                query.history_min_price = (!value.is_empty()).then(|| value.into_owned());
            }
            "max_price" => {
                query.history_max_price = (!value.is_empty()).then(|| value.into_owned());
            }
            "buyer" => query.history_buyer = normalize_optional_query(&value, 256),
            "seller" => query.history_seller = normalize_optional_query(&value, 256),
            "history_sort" => {
                if matches!(
                    value.as_ref(),
                    "closed_desc" | "closed_asc" | "price_desc" | "price_asc"
                ) {
                    query.history_sort = value.into_owned();
                }
            }
            "history_group" => {
                if matches!(value.as_ref(), "flat" | "equipment" | "buyer" | "seller") {
                    query.history_group = value.into_owned();
                }
            }
            "lot" => query.bid_log_lot = retained_query_value(&value),
            "page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    canonical_page = Some(page);
                }
            }
            "page_size" => {
                query.page_size = parse_public_page_size(&value).unwrap_or(DEFAULT_PAGE_SIZE);
            }
            "movement_page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    query.movement_page = page;
                }
            }
            "movement_page_size" => {
                query.movement_page_size =
                    parse_public_page_size(&value).unwrap_or(DEFAULT_PAGE_SIZE);
            }
            "bidlog_page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    legacy_bid_log_page = Some(page);
                }
            }
            "bid_page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    query.bid_page = page;
                }
            }
            "proxy_page" => {
                if let Ok(page) = value.parse::<i64>()
                    && page > 0
                {
                    query.proxy_page = page;
                }
            }
            _ => query.passthrough.push((key.into_owned(), value.into_owned())),
        }
    }
    query.page = canonical_page
        .or_else(|| (pathname == "/bidlog").then_some(legacy_bid_log_page).flatten())
        .unwrap_or(1);
    query.bid_log_page_alias = pathname == "/bidlog" && legacy_bid_log_page.is_some();
    query
}

fn normalize_optional_query(value: &str, maximum: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

/// lot 与 name 由页面校验，这里只去空白：非法值留给页面报字段错误且不发请求，解析时丢弃会扩大查询。
fn retained_query_value(value: &str) -> Option<String> {
    Some(value.trim()).filter(|value| !value.is_empty()).map(str::to_owned)
}

fn is_catalog_category(value: &str) -> bool {
    CATALOG_CATEGORIES.iter().any(|(category, _)| *category == value)
}

fn is_history_status(value: &str) -> bool {
    matches!(value, "live" | "final" | "settled")
}
