//! Public URL 查询状态、筛选规范化和浏览器历史辅助。

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveConnectionState {
    Checking,
    Connecting,
    Connected,
    Anonymous,
    Stopped,
    Error,
}

/// 公开拍卖看板的 URL 状态；筛选和分页必须能被刷新、分享和返回操作恢复。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicBoardQuery {
    world: Option<String>,
    auction_id: Option<String>,
    category: Option<String>,
    status: Option<String>,
    history_name: Option<String>,
    history_from: Option<String>,
    history_to: Option<String>,
    history_min_price: Option<String>,
    history_max_price: Option<String>,
    history_buyer: Option<String>,
    history_seller: Option<String>,
    bid_log_lot: Option<String>,
    page: i64,
    bid_log_page: i64,
    bid_page: i64,
    proxy_page: i64,
}

pub(crate) type PreviewHistoryLoad = Result<Option<(Option<CurrentAuction>, Vec<HistoricalAuction>)>, String>;

impl Default for PublicBoardQuery {
    fn default() -> Self {
        Self {
            world: None,
            auction_id: None,
            category: None,
            status: None,
            history_name: None,
            history_from: None,
            history_to: None,
            history_min_price: None,
            history_max_price: None,
            history_buyer: None,
            history_seller: None,
            bid_log_lot: None,
            page: 1,
            bid_log_page: 1,
            bid_page: 1,
            proxy_page: 1,
        }
    }
}

pub(crate) fn read_public_query() -> PublicBoardQuery {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return PublicBoardQuery::default();
        };
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        let mut query = PublicBoardQuery::default();
        for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
            match key.as_ref() {
                "world" => query.world = normalize_world_query(&value),
                "auction" => {
                    query.auction_id = Uuid::parse_str(&value).ok().map(|id| id.to_string())
                }
                "type" if is_catalog_category(&value) => query.category = Some(value.into_owned()),
                "status" if is_history_status(&value) => query.status = Some(value.into_owned()),
                "name" => query.history_name = normalize_optional_query(&value, 1024),
                "from" => query.history_from = normalize_optional_query(&value, 10),
                "to" => query.history_to = normalize_optional_query(&value, 10),
                "min_price" => query.history_min_price = normalize_optional_query(&value, 32),
                "max_price" => query.history_max_price = normalize_optional_query(&value, 32),
                "buyer" => query.history_buyer = normalize_optional_query(&value, 256),
                "seller" => query.history_seller = normalize_optional_query(&value, 256),
                "lot" => query.bid_log_lot = normalize_optional_query(&value, 128),
                "page" => {
                    if let Ok(page) = value.parse::<i64>() {
                        if page > 0 {
                            query.page = page;
                        }
                    }
                }
                "bidlog_page" => {
                    if let Ok(page) = value.parse::<i64>() {
                        if page > 0 {
                            query.bid_log_page = page;
                        }
                    }
                }
                "bid_page" => {
                    if let Ok(page) = value.parse::<i64>() {
                        if page > 0 {
                            query.bid_page = page;
                        }
                    }
                }
                "proxy_page" => {
                    if let Ok(page) = value.parse::<i64>() {
                        if page > 0 {
                            query.proxy_page = page;
                        }
                    }
                }
                _ => {}
            }
        }
        return query;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        PublicBoardQuery::default()
    }
}

pub(crate) fn normalize_world_query(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn normalize_optional_query(value: &str, maximum: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_catalog_category(value: &str) -> bool {
    CATALOG_CATEGORIES.iter().any(|(category, _)| *category == value)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn is_history_status(value: &str) -> bool {
    matches!(value, "live" | "final" | "settled")
}

pub(crate) fn public_query_string(query: &PublicBoardQuery) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if let Some(world) = query.world.as_deref() {
        serializer.append_pair("world", world);
    }
    if let Some(auction_id) = query.auction_id.as_deref() {
        serializer.append_pair("auction", auction_id);
    }
    if let Some(category) = query.category.as_deref() {
        serializer.append_pair("type", category);
    }
    if let Some(status) = query.status.as_deref() {
        serializer.append_pair("status", status);
    }
    for (key, value) in [
        ("name", query.history_name.as_deref()),
        ("from", query.history_from.as_deref()),
        ("to", query.history_to.as_deref()),
        ("min_price", query.history_min_price.as_deref()),
        ("max_price", query.history_max_price.as_deref()),
        ("buyer", query.history_buyer.as_deref()),
        ("seller", query.history_seller.as_deref()),
        ("lot", query.bid_log_lot.as_deref()),
    ] {
        if let Some(value) = value {
            serializer.append_pair(key, value);
        }
    }
    if query.page > 1 {
        serializer.append_pair("page", &query.page.to_string());
    }
    if query.bid_log_page > 1 {
        serializer.append_pair("bidlog_page", &query.bid_log_page.to_string());
    }
    if query.bid_page > 1 {
        serializer.append_pair("bid_page", &query.bid_page.to_string());
    }
    if query.proxy_page > 1 {
        serializer.append_pair("proxy_page", &query.proxy_page.to_string());
    }
    let encoded = serializer.finish();
    if encoded.is_empty() { String::new() } else { format!("?{encoded}") }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn set_public_query(query: &PublicBoardQuery, replace: bool) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(history) = window.history() else {
        return;
    };
    let pathname = window.location().pathname().unwrap_or_else(|_| "/".to_owned());
    let url = format!("{pathname}{}", public_query_string(query));
    if replace {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(&url));
    } else {
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some(&url));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_public_query(_query: &PublicBoardQuery, _replace: bool) {}

pub(crate) fn update_public_query(update: impl FnOnce(&mut PublicBoardQuery), replace: bool) {
    let mut query = read_public_query();
    update(&mut query);
    set_public_query(&query, replace);
}

pub(crate) fn public_href(path: &str, query: &PublicBoardQuery) -> String {
    format!("{path}{}", public_query_string(query))
}

/// 把旧版公开端别名无历史记录地收敛到 canonical URL，并保留筛选/分页查询参数。
///
/// 别名本身仍然注册在路由表中，旧链接可以继续打开；跳转使用 `replace`，避免用户按返回
/// 键时再次落回别名页面。查询字符串由浏览器当前地址读取，不把 URL 状态复制到组件状态。
pub(crate) fn redirect_public_alias(path: &'static str) -> Element {
    let target = format!("{path}{}", current_browser_search());
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_browser_search() -> String {
    web_sys::window().and_then(|window| window.location().search().ok()).unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_browser_search() -> String {
    String::new()
}

pub(crate) const CATALOG_CATEGORIES: [(&str, &str); 7] = [
    ("one", "One"),
    ("two", "Two"),
    ("sta", "Sta"),
    ("shd", "Shd"),
    ("clo", "Clo"),
    ("lig", "Lig"),
    ("hea", "Hea"),
];
