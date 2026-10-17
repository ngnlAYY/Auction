//! Public URL 查询状态、筛选规范化和浏览器历史辅助。

use super::*;

/// Router 首次挂载前的公开端地址快照，用于恢复未在 `Routable` 中声明的查询参数。
#[derive(Clone)]
pub(crate) struct InitialPublicLocation {
    #[cfg(target_arch = "wasm32")]
    pathname: String,
    #[cfg(target_arch = "wasm32")]
    search: String,
    #[cfg(target_arch = "wasm32")]
    had_query_marker: bool,
    available: Rc<Cell<bool>>,
}

impl InitialPublicLocation {
    #[cfg(target_arch = "wasm32")]
    fn search_for(&self, pathname: &str) -> Option<String> {
        (self.available.replace(false) && self.pathname == pathname && !self.search.is_empty())
            .then(|| self.search.clone())
    }

    pub(crate) fn expire(&self) {
        #[cfg(target_arch = "wasm32")]
        self.remove_router_empty_query();
        self.available.set(false);
    }

    /// Dioxus 0.7 会把带可选 query 的裸 `/login` 首次规范化成 `/login?`；旧 Router 保持
    /// 原地址。这里只在原始 URL 没有 `?`、pathname/search 仍未变化时无历史记录地移除空标记。
    #[cfg(target_arch = "wasm32")]
    fn remove_router_empty_query(&self) {
        if !self.search.is_empty() || self.had_query_marker {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let location = window.location();
        if location.pathname().ok().as_deref() != Some(self.pathname.as_str())
            || location.search().is_ok_and(|search| !search.is_empty())
        {
            return;
        }
        let Ok(href) = location.href() else {
            return;
        };
        if !href.split('#').next().is_some_and(|url| url.ends_with('?')) {
            return;
        }
        let hash = location.hash().unwrap_or_default();
        let url = format!("{}{hash}", self.pathname);
        if let Ok(history) = window.history() {
            let state = history.state().unwrap_or(JsValue::NULL);
            let _ = history.replace_state_with_url(&state, "", Some(&url));
        }
    }
}

/// 在 Dioxus Router 规范化未声明的 query 前保存首次 pathname 与 search。
pub(crate) fn initial_public_location() -> InitialPublicLocation {
    #[cfg(target_arch = "wasm32")]
    let (pathname, search, had_query_marker) = web_sys::window()
        .map(|window| {
            let location = window.location();
            let had_query_marker = location
                .href()
                .ok()
                .and_then(|href| href.split('#').next().map(str::to_owned))
                .is_some_and(|url| url.contains('?'));
            (
                location.pathname().unwrap_or_else(|_| "/".to_owned()),
                location.search().unwrap_or_default(),
                had_query_marker,
            )
        })
        .unwrap_or_else(|| ("/".to_owned(), String::new(), false));

    InitialPublicLocation {
        #[cfg(target_arch = "wasm32")]
        pathname,
        #[cfg(target_arch = "wasm32")]
        search,
        #[cfg(target_arch = "wasm32")]
        had_query_marker,
        available: Rc::new(Cell::new(true)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveConnectionState {
    Connecting,
    Degraded,
    Live,
    Manual,
}

/// 公开拍卖看板的 URL 状态；筛选和分页必须能被刷新、分享和返回操作恢复。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PublicBoardQuery {
    pub(crate) world: Option<String>,
    pub(crate) auction_id: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) history_name: Option<String>,
    pub(crate) history_from: Option<String>,
    pub(crate) history_to: Option<String>,
    pub(crate) history_min_price: Option<String>,
    pub(crate) history_max_price: Option<String>,
    pub(crate) history_buyer: Option<String>,
    pub(crate) history_seller: Option<String>,
    pub(crate) bid_log_lot: Option<String>,
    pub(crate) page: i64,
    pub(crate) page_size: i64,
    /// 补偿池展开流水的独立分页；批次分页与流水分页不能互相覆盖。
    pub(crate) movement_page: i64,
    pub(crate) movement_page_size: i64,
    /// 是否从迁移期间写出的 `bidlog_page` 别名恢复了竞价日志页码。
    pub(crate) bid_log_page_alias: bool,
    pub(crate) bid_page: i64,
    pub(crate) proxy_page: i64,
    /// 当前页面不拥有的 query 对必须原样保留，避免筛选或翻页时删除外部入口参数。
    pub(crate) passthrough: Vec<(String, String)>,
}

pub(crate) type PreviewHistoryLoad =
    Result<Option<(Option<CurrentAuction>, Vec<HistoricalAuction>)>, String>;

/// 浏览器 pathname/query 变化版本；查询驱动页面通过独立类型订阅，避免与数据刷新信号混淆。
#[derive(Clone, Copy)]
pub(crate) struct PublicLocationRevision(pub(crate) Signal<u64>);

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
            page_size: stored_public_page_size(false),
            movement_page: 1,
            movement_page_size: stored_public_page_size(true),
            bid_log_page_alias: false,
            bid_page: 1,
            proxy_page: 1,
            passthrough: Vec::new(),
        }
    }
}

pub(crate) fn read_public_query() -> PublicBoardQuery {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return PublicBoardQuery::default();
        };
        let pathname = window.location().pathname().unwrap_or_default();
        let mut search = window.location().search().unwrap_or_default();
        if search.is_empty()
            && let Some(initial) = try_consume_context::<InitialPublicLocation>()
            && let Some(initial_search) = initial.search_for(&pathname)
        {
            // Router 只拥有 pathname；恢复原 query 时沿用它建立的 history.state，避免破坏
            // Dioxus 的滚动位置记录，也不额外增加浏览器历史条目。
            if let Ok(history) = window.history() {
                let state = history.state().unwrap_or(JsValue::NULL);
                let url = format!("{pathname}{initial_search}");
                if history.replace_state_with_url(&state, "", Some(&url)).is_ok() {
                    search = initial_search;
                }
            }
        }
        parse_public_query(&pathname, &search)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        PublicBoardQuery::default()
    }
}

#[cfg(target_arch = "wasm32")]
fn parse_public_query(pathname: &str, search: &str) -> PublicBoardQuery {
    let raw = search.strip_prefix('?').unwrap_or_default();
    let mut query = PublicBoardQuery::default();
    let mut canonical_page = None;
    let mut legacy_bid_log_page = None;
    for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
        match key.as_ref() {
            "world" => query.world = normalize_world_query(&value),
            "auction" => query.auction_id = Uuid::parse_str(&value).ok().map(|id| id.to_string()),
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

pub(crate) fn normalize_world_query(value: &str) -> Option<String> {
    match value.trim() {
        "main" => Some("main".to_owned()),
        "isekai" => Some("isekai".to_owned()),
        _ => None,
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
    let pairs = public_query_pairs(query);
    if pairs.is_empty() {
        return String::new();
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in &pairs {
        serializer.append_pair(key, value);
    }
    format!("?{}", serializer.finish())
}

/// 序列化公开端查询状态；数量即使为 10 也显式保留，防止本地偏好改变分享及历史语义。
pub(crate) fn public_query_pairs(query: &PublicBoardQuery) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if let Some(world) = query.world.as_deref() {
        pairs.push(("world".to_owned(), world.to_owned()));
    }
    if let Some(auction_id) = query.auction_id.as_deref() {
        pairs.push(("auction".to_owned(), auction_id.to_owned()));
    }
    if let Some(category) = query.category.as_deref() {
        pairs.push(("type".to_owned(), category.to_owned()));
    }
    if let Some(status) = query.status.as_deref() {
        pairs.push(("status".to_owned(), status.to_owned()));
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
            pairs.push((key.to_owned(), value.to_owned()));
        }
    }
    if query.page > 1 {
        pairs.push(("page".to_owned(), query.page.to_string()));
    }
    pairs.push(("page_size".to_owned(), query.page_size.to_string()));
    if query.movement_page > 1 {
        pairs.push(("movement_page".to_owned(), query.movement_page.to_string()));
    }
    pairs.push(("movement_page_size".to_owned(), query.movement_page_size.to_string()));
    if query.bid_page > 1 {
        pairs.push(("bid_page".to_owned(), query.bid_page.to_string()));
    }
    if query.proxy_page > 1 {
        pairs.push(("proxy_page".to_owned(), query.proxy_page.to_string()));
    }
    for (key, value) in &query.passthrough {
        pairs.push((key.clone(), value.clone()));
    }
    pairs
}

/// 本 crate 在地址栏管理的全部公开 query 键；含迁移期间曾写出的 `bidlog_page` 别名，
/// 写回时该别名被清除（序列化只产出 canonical `page`）。
const PUBLIC_QUERY_KEYS: &[&str] = &[
    "world",
    "auction",
    "type",
    "status",
    "name",
    "from",
    "to",
    "min_price",
    "max_price",
    "buyer",
    "seller",
    "lot",
    "page",
    "page_size",
    "movement_page",
    "movement_page_size",
    "bidlog_page",
    "bid_page",
    "proxy_page",
];

/// 挂载与地址栏双向同步的公开端查询状态。
/// 初值与浏览器前进/后退（popstate）恢复共用 `read_public_query` 的既有解析（含 Router
/// 规范化前保存的首次 query 恢复）；写回只替换本 crate 管理的键，未声明的外部入口参数
/// 原样保留。每个页面挂载自己的实例，同时订阅外壳程序化导航的位置版本；外壳另有独立
/// 实例承担 world 键的 popstate 恢复。
pub(crate) fn use_public_query_state() -> QueryState<PublicBoardQuery> {
    let query =
        use_query_state(PUBLIC_QUERY_KEYS, |_| read_public_query(), managed_public_query_pairs);
    let location_revision = try_consume_context::<PublicLocationRevision>();
    let mut state = query.state;
    use_effect(move || {
        let Some(PublicLocationRevision(revision)) = location_revision else { return };
        let _ = revision();
        // pushState 不触发 popstate；首页清理查询后，已挂载目录仍须同步到新的地址栏状态。
        let current = read_public_query();
        if *state.peek() != current {
            state.set(current);
        }
    });
    query
}

/// 把完整查询状态按既有序列化规则写回地址栏（只替换本 crate 管理的键，其余键原样保留）。
pub(crate) fn write_public_query(query: &PublicBoardQuery, mode: QueryWriteMode) {
    write_query_pairs(PUBLIC_QUERY_KEYS, &managed_public_query_pairs(query), mode);
}

/// 原地写回时未知参数由共享层保留一次；完整链接仍通过 `public_query_pairs` 携带 passthrough。
fn managed_public_query_pairs(query: &PublicBoardQuery) -> Vec<(String, String)> {
    public_query_pairs(query)
        .into_iter()
        .filter(|(key, _)| PUBLIC_QUERY_KEYS.contains(&key.as_str()))
        .collect()
}

/// 读取地址栏最新查询、应用变更并按模式写回（等价旧 `update_public_query` 的读-改-写），
/// 同时把给定的 hook 状态信号同步到新值。接收 `Signal`（Copy）而非整个 hook 句柄，
/// 页面内多个事件闭包可以直接共享。
pub(crate) fn update_public_query_state(
    mut state: Signal<PublicBoardQuery>,
    update: impl FnOnce(&mut PublicBoardQuery),
    mode: QueryWriteMode,
) {
    let mut query = read_public_query();
    update(&mut query);
    write_public_query(&query, mode);
    state.set(query);
}

/// 当前地址栏 pathname；非 WASM 构建恒为 `/`（仅服务静态检查路径）。
pub(crate) fn current_public_pathname() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.location().pathname().ok())
            .unwrap_or_else(|| "/".to_owned())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        "/".to_owned()
    }
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
    let state = browser_scroll_state(&window);
    if replace {
        let _ = history.replace_state_with_url(&state, "", Some(&url));
    } else {
        // 与 Dioxus WebHistory 一致，先把离开的条目更新为当前滚动位置，再建立新条目。
        let _ = history.replace_state_with_url(&state, "", None);
        let _ = history.push_state_with_url(&state, "", Some(&url));
    }
}

#[cfg(target_arch = "wasm32")]
fn browser_scroll_state(window: &web_sys::Window) -> js_sys::Array {
    let state = js_sys::Array::new();
    state.push(&JsValue::from_f64(window.scroll_x().unwrap_or_default()));
    state.push(&JsValue::from_f64(window.scroll_y().unwrap_or_default()));
    state
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_public_query(_query: &PublicBoardQuery, _replace: bool) {}

pub(crate) fn public_href(path: &str, query: &PublicBoardQuery) -> String {
    format!("{path}{}", public_query_string(query))
}

/// 首页入口不冻结数量：导航时读取最新本地偏好，避免外壳旧链接覆盖刚保存的选择。
pub(crate) fn public_auction_home_href(world: &str) -> String {
    if world == "isekai" { "/?world=isekai" } else { "/" }.to_owned()
}

/// 生成当前世界的拍卖首页查询；同路由导航可直接用它替换历史视图状态。
pub(crate) fn public_auction_home_query(world: &str) -> PublicBoardQuery {
    // 只保留目标世界与本地数量偏好，避免从其他页面带回不属于首页的筛选和页码。
    PublicBoardQuery {
        world: (world == "isekai").then(|| "isekai".to_owned()),
        ..PublicBoardQuery::default()
    }
}

/// 把旧版公开端别名无历史记录地收敛到 canonical URL，并保留筛选/分页查询参数。
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
