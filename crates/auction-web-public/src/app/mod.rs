//! 公开端浏览器界面（Dioxus CSR）。
//!
//! 路由集合按旧版 Auction-BF 的 `manifests/public.routes.json` 冻结，保持 URL 契约不变。
//! 界面文案全部经由 `dioxus-i18n` 取自 `i18n/*.ftl`，默认英文；受支持语言与持久化行为
//! 由 `auction-web-shared` 统一提供。
//!
//! 本 crate 只允许依赖 `auction-contracts` 的稳定 DTO 与 `auction-web-shared` 的基础能力，
//! 不得导入存储、配置或凭据访问路径。

mod account;
mod blacklist;
mod commands;
mod compensation;
mod forum;
mod pagination;
mod proxy;
mod reads;
mod realtime;
mod rules;
mod session;

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use gloo_timers::future::TimeoutFuture;
use std::{cell::Cell, collections::BTreeMap, rc::Rc};
use unic_langid::langid;
use url::form_urlencoded;
#[cfg(target_arch = "wasm32")]
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

use crate::api::{
    AuctionListPage, CatalogRow, CurrentAuction, EquipmentDetail, HistoricalAuction, LotDetailItem,
    ProxyCommandAuthorization, RealtimeEvent, WorldStatus, Worlds, estimated_now,
    fetch_auction_list, fetch_auction_page, fetch_current_auction, fetch_current_lots,
    fetch_historical_auction, fetch_lot_bid_log, fetch_lot_detail, fetch_my_proxy,
    fetch_public_session, fetch_worlds, lot_code_for, subscribe_auction_realtime,
    subscribe_realtime,
};
use auction_web_shared::{
    BaseStylesheet, LanguagePicker, format_credit_amount as format_amount, format_localized_time,
    i18n::{DEFAULT_PUBLIC_LOCALE, initial_language},
    use_dialog_focus_trap, use_locale_effects,
};
use pagination::PublicPager;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiveConnectionState {
    Checking,
    Connecting,
    Connected,
    Anonymous,
    Stopped,
    Error,
}

/// 公开拍卖看板的 URL 状态；筛选和分页必须能被刷新、分享和返回操作恢复。
#[derive(Clone, Debug, PartialEq, Eq)]
struct PublicBoardQuery {
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

type PreviewHistoryLoad = Result<Option<(Option<CurrentAuction>, Vec<HistoricalAuction>)>, String>;

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

fn read_public_query() -> PublicBoardQuery {
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

fn normalize_world_query(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(target_arch = "wasm32")]
fn normalize_optional_query(value: &str, maximum: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(target_arch = "wasm32")]
fn is_catalog_category(value: &str) -> bool {
    CATALOG_CATEGORIES.iter().any(|(category, _)| *category == value)
}

#[cfg(target_arch = "wasm32")]
fn is_history_status(value: &str) -> bool {
    matches!(value, "live" | "final" | "settled")
}

fn public_query_string(query: &PublicBoardQuery) -> String {
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
fn set_public_query(query: &PublicBoardQuery, replace: bool) {
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
fn set_public_query(_query: &PublicBoardQuery, _replace: bool) {}

fn update_public_query(update: impl FnOnce(&mut PublicBoardQuery), replace: bool) {
    let mut query = read_public_query();
    update(&mut query);
    set_public_query(&query, replace);
}

fn public_href(path: &str, query: &PublicBoardQuery) -> String {
    format!("{path}{}", public_query_string(query))
}

/// 把旧版公开端别名无历史记录地收敛到 canonical URL，并保留筛选/分页查询参数。
///
/// 别名本身仍然注册在路由表中，旧链接可以继续打开；跳转使用 `replace`，避免用户按返回
/// 键时再次落回别名页面。查询字符串由浏览器当前地址读取，不把 URL 状态复制到组件状态。
fn redirect_public_alias(path: &'static str) -> Element {
    let target = format!("{path}{}", current_browser_search());
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn current_browser_search() -> String {
    web_sys::window().and_then(|window| window.location().search().ok()).unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn current_browser_search() -> String {
    String::new()
}

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset =
    asset!("/assets/public.css", AssetOptions::css().with_preload(true).with_static_head(true));

pub fn run() {
    dioxus::launch(App);
}

/// 公开端路由表。
///
/// 路径逐条对应旧系统已冻结的公开端 route，新增路由前必须先确认外部契约。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
enum Route {
    #[layout(PublicShell)]
        #[route("/")]
        AuctionIndex {},

        #[route("/auction")]
        AuctionPage {},

        #[route("/auctions")]
        AuctionList {},

        #[route("/lots/:lot_code")]
        LotDetail { lot_code: String },

        #[route("/bidlog")]
        BidLog {},

        #[route("/history")]
        History {},

        #[route("/login")]
        Login {},

        #[route("/me")]
        Me {},

        #[route("/account")]
        Account {},

        #[route("/rules")]
        Rules {},

        #[route("/blacklist")]
        Blacklist {},

        #[route("/compensation-pool")]
        CompensationPool {},

        #[route("/compensation")]
        Compensation {},

        #[route("/status")]
        Status {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
fn App() -> Element {
    // 语言资源在编译期内嵌，避免首屏额外请求；默认英文，并沿用已保存的语言偏好。
    let i18n = use_init_i18n(|| {
        I18nConfig::new(initial_language(DEFAULT_PUBLIC_LOCALE))
            .with_locale((langid!("en"), include_str!("../../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../../i18n/ko.ftl")))
            .with_fallback(DEFAULT_PUBLIC_LOCALE)
    });
    use_locale_effects(i18n);

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: PUBLIC_CSS }
        document::Title { {t!("app-title")} }
        Router::<Route> {}
    }
}

/// 公开端外壳：沿用旧版 Auction-BF 的单行页头、导航和居中内容区。
#[component]
fn PublicShell() -> Element {
    use_dialog_focus_trap();
    let selected_world =
        use_signal(|| read_public_query().world.unwrap_or_else(|| "main".to_owned()));
    let mut refresh_generation = use_signal(|| 0_u64);
    let realtime_state = use_signal(|| LiveConnectionState::Checking);
    let realtime_cursor = use_signal(|| 0_i64);
    use_context_provider(|| selected_world);
    use_context_provider(|| refresh_generation);
    use_context_provider(|| realtime_state);

    let _realtime = use_future({
        let refresh_generation = refresh_generation;
        let mut realtime_state = realtime_state;
        move || async move {
            loop {
                realtime_state.set(LiveConnectionState::Checking);
                match fetch_public_session().await {
                    Ok(Some(session)) if session.authenticated => {
                        realtime_state.set(LiveConnectionState::Connecting);
                        let mut callback_state = realtime_state;
                        let mut callback_refresh = refresh_generation;
                        let after = realtime_cursor();
                        let mut callback_cursor = realtime_cursor;
                        match subscribe_realtime(after, move |page| {
                            callback_state.set(LiveConnectionState::Connected);
                            let refetch_required = page.refetch_required;
                            callback_cursor.set(if refetch_required { 0 } else { page.next_after });
                            if refetch_required
                                || page.events.iter().any(should_refresh_auction_board)
                            {
                                callback_refresh.set(callback_refresh().saturating_add(1));
                            }
                        })
                        .await
                        {
                            Ok(()) => realtime_state.set(LiveConnectionState::Stopped),
                            Err(_) => realtime_state.set(LiveConnectionState::Error),
                        }
                    }
                    Ok(_) => realtime_state.set(LiveConnectionState::Anonymous),
                    Err(_) => realtime_state.set(LiveConnectionState::Error),
                }
                TimeoutFuture::new(5_000).await;
            }
        }
    });
    let connection_state = realtime_state();
    let connection_class = live_connection_class(connection_state);
    let connection_message = realtime::live_connection_message(connection_state);
    let can_manual_refresh = matches!(
        connection_state,
        LiveConnectionState::Anonymous | LiveConnectionState::Stopped | LiveConnectionState::Error
    );

    rsx! {
        div { class: "public-shell",
            a { class: "public-shell__skip", href: "#public-content", {t!("skip")} }
            header { class: "public-shell__header",
                Link { class: "public-shell__logo", to: Route::AuctionIndex {},
                    {t!("app-title")}
                }
                nav { class: "public-shell__nav", aria_label: t!("nav-label"),
                    Link { to: Route::AuctionIndex {}, {t!("nav-auctions")} }
                    Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
                    Link { to: Route::AuctionList {}, {t!("page-auctions-title")} }
                    Link { to: Route::History {}, {t!("page-history-title")} }
                    Link { to: Route::Rules {}, {t!("page-rules-title")} }
                    Link { to: Route::Blacklist {}, {t!("page-blacklist-title")} }
                    Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
                }
                div { class: "public-shell__controls",
                    WorldPicker { selected_world }
                    span {
                        class: "public-connection-badge {connection_class}",
                        role: "status",
                        aria_label: t!("realtime-connection-label"),
                        {connection_message}
                    }
                    if can_manual_refresh {
                        button {
                            class: "public-connection-refresh",
                            r#type: "button",
                            onclick: move |_| {
                                refresh_generation.set(refresh_generation().saturating_add(1));
                            },
                            {t!("realtime-refresh")}
                        }
                    }
                    session::SessionMenu {}
                    Link {
                        class: "public-shell__utility-link public-shell__utility-link--status",
                        to: Route::Status {},
                        {t!("nav-status")}
                    }
                    LanguagePicker { label: t!("language-label") }
                }
            }
            main { class: "public-shell__content",
                Outlet::<Route> {}
            }
        }
    }
}

#[component]
fn AuctionIndex() -> Element {
    rsx! {
        AuctionBoard {}
    }
}

#[component]
fn AuctionPage() -> Element {
    redirect_public_alias("/")
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不再渲染迁移阶段示例数据。
#[component]
fn AuctionBoard() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let refresh_generation = use_context::<Signal<u64>>();
    let query = read_public_query();
    let requested_auction = query.auction_id.clone();
    let historical = requested_auction.is_some();
    let current = use_resource(move || {
        let _ = refresh_generation();
        let world = selected_world();
        let requested_auction = requested_auction.clone();
        async move {
            match requested_auction {
                Some(auction_id) => fetch_historical_auction(auction_id).await.map(Some),
                None => fetch_current_auction(world).await,
            }
        }
    });

    match &*current.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(None)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(auction))) => rsx! {
            AuctionData {
                auction: auction.clone(),
                historical,
                initial_category: query.category.clone().unwrap_or_else(|| "all".to_owned()),
                initial_page: query.page,
            }
        },
    }
}

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
#[component]
fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let worlds = use_resource(fetch_worlds);
    let options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let current = selected_world();
    let active = options
        .iter()
        .find(|world| **world == current)
        .cloned()
        .unwrap_or_else(|| options.first().cloned().unwrap_or_else(|| "main".to_owned()));
    let active_for_effect = active.clone();
    use_effect(move || {
        if selected_world() != active_for_effect {
            selected_world.set(active_for_effect.clone());
        }
    });
    let options_empty = options.is_empty();

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                value: active.clone(),
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    selected_world.set(world.clone());
                    update_public_query(
                        |query| {
                            query.world = normalize_world_query(&world);
                            query.auction_id = None;
                        },
                        true,
                    );
                },
                for world in options {
                    option { value: world.clone(), {world_label(&world)} }
                }
            }
        }
    }
}

fn enabled_worlds(worlds: &Worlds) -> Vec<String> {
    worlds
        .worlds
        .iter()
        .filter(|(_, status)| status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect()
}

fn world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

fn live_connection_class(state: LiveConnectionState) -> &'static str {
    match state {
        LiveConnectionState::Checking => "public-connection-badge--checking",
        LiveConnectionState::Connecting => "public-connection-badge--connecting",
        LiveConnectionState::Connected => "public-connection-badge--connected",
        LiveConnectionState::Anonymous => "public-connection-badge--anonymous",
        LiveConnectionState::Stopped => "public-connection-badge--stopped",
        LiveConnectionState::Error => "public-connection-badge--error",
    }
}

fn should_refresh_auction_board(event: &RealtimeEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "auction.created"
            | "auction.preview_updated"
            | "preview_catalog_changed"
            | "auction.opened"
            | "auction.final_phase_entered"
            | "auction.cancelled"
            | "auction.settling"
            | "auction.settled"
            | "lot.bid_accepted"
    )
}

const CATALOG_CATEGORIES: [(&str, &str); 7] = [
    ("one", "One"),
    ("two", "Two"),
    ("sta", "Sta"),
    ("shd", "Shd"),
    ("clo", "Clo"),
    ("lig", "Lig"),
    ("hea", "Hea"),
];

#[component]
fn AuctionData(
    auction: CurrentAuction,
    historical: bool,
    initial_category: String,
    initial_page: i64,
) -> Element {
    let mut clock = use_signal(estimated_now);
    let active_category = use_signal(|| initial_category.clone());
    let page_number = use_signal(|| initial_page.max(1));
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let should_refresh_at_open = !historical && auction.presentation_mode != "live";
    let opening_at =
        DateTime::parse_from_rfc3339(&auction.opens_at).ok().map(|value| value.with_timezone(&Utc));
    let opening_key = format!("{}:{}", auction.id, auction.opens_at);
    let mut opening_refresh_key = use_signal(|| None::<String>);
    let mut opening_refreshed = use_signal(|| false);
    let _clock_task = use_future({
        let opening_key = opening_key.clone();
        move || {
            let opening_key = opening_key.clone();
            async move {
                loop {
                    TimeoutFuture::new(1_000).await;
                    let now = estimated_now();
                    clock.set(now);
                    if opening_refresh_key().as_deref() != Some(opening_key.as_str()) {
                        opening_refresh_key.set(Some(opening_key.clone()));
                        opening_refreshed.set(false);
                    }
                    if should_refresh_at_open
                        && !opening_refreshed()
                        && realtime_state() == LiveConnectionState::Connected
                        && opening_at.is_some_and(|opens_at| now >= opens_at)
                    {
                        opening_refreshed.set(true);
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    }
                }
            }
        }
    });
    let auction_id = auction.id.clone();
    let session = use_resource(fetch_public_session);
    let viewer_name = match &*session.read_unchecked() {
        Some(Ok(Some(session))) => Some(session.username.clone()),
        _ => None,
    };
    let authenticated = matches!(&*session.read_unchecked(), Some(Ok(Some(_))));
    let history_switch = use_resource({
        let world = auction.world.clone();
        let should_load = historical || auction.presentation_mode != "live";
        move || {
            let world = world.clone();
            async move {
                if !should_load {
                    return Ok(None);
                }
                let current = fetch_current_auction(world).await?;
                let rounds = fetch_auction_list().await?;
                Ok(Some((current, rounds)))
            }
        }
    });
    let _auction_realtime = use_future({
        let realtime_auction_id = auction_id.clone();
        move || {
            let realtime_auction_id = realtime_auction_id.clone();
            async move {
                if historical {
                    return;
                }
                let mut after = 0_i64;
                loop {
                    let cursor = Rc::new(Cell::new(after));
                    let callback_cursor = Rc::clone(&cursor);
                    let mut callback_refresh = refresh_generation;
                    let result = subscribe_auction_realtime(
                        after,
                        realtime_auction_id.clone(),
                        move |page| {
                            let refetch_required = page.refetch_required;
                            callback_cursor.set(if refetch_required { 0 } else { page.next_after });
                            if refetch_required
                                || page.events.iter().any(should_refresh_auction_board)
                            {
                                callback_refresh.set(callback_refresh().saturating_add(1));
                            }
                        },
                    )
                    .await;
                    after = cursor.get();
                    let _ = result;
                    TimeoutFuture::new(5_000).await;
                }
            }
        }
    });
    let mut lots_refresh_generation = use_signal(|| 0_u64);
    let lots = use_resource(move || {
        let _ = refresh_generation();
        let _ = lots_refresh_generation();
        let auction_id = auction_id.clone();
        let category = active_category();
        let page = page_number();
        let category = (category != "all").then_some(category);
        async move { fetch_current_lots(auction_id, category, page).await }
    });
    let aligned = match &*lots.read_unchecked() {
        Some(Ok(page)) => {
            page.auction_id == auction.id
                && page.world == auction.world
                && page.number == auction.number
                && page.auction_status == auction.status
                && page.presentation_mode == auction.presentation_mode
                && page.bidding_open == auction.bidding_open
                && page.preview_revision == auction.preview_revision
        }
        _ => true,
    };
    let alignment_key = format!(
        "{}:{}:{}:{}:{}",
        auction.id,
        auction.status,
        auction.presentation_mode,
        auction.bidding_open,
        auction.preview_revision
    );
    let mut alignment_attempt = use_signal(|| None::<String>);
    use_effect(move || {
        if aligned {
            if alignment_attempt().is_some() {
                alignment_attempt.set(None);
            }
        } else if alignment_attempt().as_deref() != Some(alignment_key.as_str()) {
            alignment_attempt.set(Some(alignment_key.clone()));
            lots_refresh_generation.set(lots_refresh_generation().saturating_add(1));
        }
    });
    let status_label = auction_status_label(&auction.status);
    let now = clock();
    let is_live_presentation = auction.presentation_mode == "live";
    let auction_end = if is_live_presentation {
        auction.scheduled_ends_at.clone()
    } else {
        Some(auction.opens_at.clone())
    };
    let auction_countdown = auction_end.as_deref().map(|ends_at| countdown_display(ends_at, now));
    let catalog = match &*lots.read_unchecked() {
        None => {
            rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-loading")} } }
        }
        Some(Err(_)) => {
            rsx! {
                RetryPlaceholder {
                    title: t!("auction-title"),
                    detail: t!("data-error"),
                    refresh_generation: lots_refresh_generation,
                }
            }
        }
        Some(Ok(_)) if !aligned => rsx! {
            RetryPlaceholder {
                title: t!("auction-title"),
                detail: t!("data-aligning"),
                refresh_generation: lots_refresh_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            CatalogTable {
                rows: page.rows(),
                now,
                projection_kind: page.projection_kind.clone(),
                category_counts: page.category_counts.clone().unwrap_or_default(),
                active_category,
                page_number,
                page_total: page.total.unwrap_or_else(|| page.rows().len() as i64),
                page_size: if page.page_size > 0 { page.page_size } else { 20 },
                can_bid: is_live_presentation && auction.bidding_open,
                allow_slow: auction.status == "live",
                authenticated,
                viewer_name: viewer_name.clone(),
            }
        },
    };

    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            if historical {
                HistoricalAuctionNavigation { auction: auction.clone() }
            }
            header { class: "public-auction-round",
                div { class: "public-auction-round__title",
                    h1 { id: "auction-title", {t!("auction-title")}, " #{auction.number}" }
                    span { class: "public-auction-round__world", "{auction_world_label(&auction.world)}" }
                    span {
                        class: "public-auction-round__status public-auction-round__status--{auction_status_class(&auction.status)}",
                        "{status_label}"
                    }
                }
                if let (Some(scheduled_ends_at), Some(countdown)) =
                    (auction_end.clone(), auction_countdown)
                {
                    p { class: "public-auction-round__end",
                        if is_live_presentation { {t!("auction-ends-at")} } else { {t!("auction-opens-at")} }
                        time { datetime: scheduled_ends_at.clone(), "{format_auction_time(&scheduled_ends_at)}" }
                        span {
                            class: countdown_class(&countdown),
                            aria_label: t!("auction-countdown"),
                            "{countdown.label}"
                        }
                    }
                }
                {preview_history_switch(&history_switch.read_unchecked(), &auction, historical)}
            }

            {catalog}
            if !historical && is_live_presentation {
                {forum::panel(auction.id.clone())}
            }
        }
    }
}

fn preview_history_switch(
    state: &Option<PreviewHistoryLoad>,
    auction: &CurrentAuction,
    historical: bool,
) -> Element {
    if !historical && auction.presentation_mode == "live" {
        return rsx! {};
    }

    let Some(state) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-loading")} }
                }
            }
        };
    };
    let Ok(Some((current, rounds))) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-error")} }
                }
            }
        };
    };
    let Some(current) = current else {
        return rsx! {};
    };
    if current.presentation_mode == "live" {
        return rsx! {};
    }

    let latest_settled = rounds
        .iter()
        .filter(|round| round.world_id == current.world)
        .max_by_key(|round| round.sequence);
    if historical && latest_settled.is_none_or(|round| round.id != auction.id) {
        return rsx! {};
    }

    let mut current_query = read_public_query();
    current_query.auction_id = None;
    current_query.world = normalize_world_query(&current.world);
    let current_href = public_href("/auction", &current_query);
    let previous = rounds
        .iter()
        .filter(|round| round.world_id == current.world && round.sequence < current.number)
        .max_by_key(|round| round.sequence);
    let previous_href = previous.map(|round| {
        let mut query = read_public_query();
        query.auction_id = Some(round.id.clone());
        query.world = normalize_world_query(&round.world_id);
        public_href("/auction", &query)
    });

    rsx! {
        nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
            span { class: "public-auction-view-switch__options",
                if historical {
                    a { href: current_href, {t!("auction-history-current")} }
                } else if let Some(previous_href) = previous_href {
                    a { href: previous_href, {t!("auction-history-previous-view")} }
                } else {
                    span { {t!("auction-history-no-previous")} }
                }
            }
        }
    }
}

/// 历史拍卖页的返回和上一轮导航；当前轮次不显示历史导航，避免污染首屏竞价操作。
#[component]
fn HistoricalAuctionNavigation(auction: CurrentAuction) -> Element {
    let history = use_resource(fetch_auction_list);
    let previous = match &*history.read_unchecked() {
        Some(Ok(rows)) => rows
            .iter()
            .filter(|row| row.world_id == auction.world && row.sequence < auction.number)
            .max_by_key(|row| row.sequence)
            .cloned(),
        _ => None,
    };

    rsx! {
        nav {
            class: "public-auction-history-nav",
            aria_label: t!("auction-history-navigation"),
            a { href: "/auctions", {t!("auction-history-back")} }
            if let Some(previous) = previous {
                a {
                    href: historical_auction_href(&previous.id, Some(&previous.world_id)),
                    {t!("auction-history-previous", number: previous.sequence)}
                }
            }
        }
    }
}

#[component]
fn CatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    projection_kind: String,
    category_counts: BTreeMap<String, i64>,
    mut active_category: Signal<String>,
    mut page_number: Signal<i64>,
    page_total: i64,
    page_size: i64,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let active = active_category();
    let groups = grouped_catalog_rows(&rows, &active);
    let is_live = projection_kind == "live";
    let all_count = catalog_count(&category_counts, "all", &rows);

    rsx! {
        nav { class: "public-auction-filter", aria_label: t!("filter-label"),
            button {
                class: "public-auction-filter__tab",
                r#type: "button",
                aria_pressed: active == "all",
                onclick: move |_| {
                    active_category.set("all".to_owned());
                    page_number.set(1);
                    update_public_query(
                        |query| {
                            query.category = None;
                            query.page = 1;
                        },
                        false,
                    );
                },
                {t!("filter-all")}
                span { class: "public-auction-filter__count", "{all_count}" }
            }
            for (category, _) in CATALOG_CATEGORIES {
                button {
                    class: "public-auction-filter__tab",
                    r#type: "button",
                    aria_pressed: active == category,
                    onclick: move |_| {
                        active_category.set(category.to_owned());
                        page_number.set(1);
                        update_public_query(
                            |query| {
                                query.category = Some(category.to_owned());
                                query.page = 1;
                            },
                            false,
                        );
                    },
                    {catalog_category_label(category)}
                    span { class: "public-auction-filter__count", "{catalog_count(&category_counts, category, &rows)}" }
                }
            }
        }
        if groups.is_empty() {
            div { class: "public-auction-empty",
                h2 { {t!("catalog-empty")} }
            }
        }
        for (category, category_rows) in groups {
            section { class: "public-auction-category",
                h2 { class: "public-auction-category__title",
                    {catalog_category_label(category)}
                    span { class: "public-auction-category__code", "{catalog_category_code(category)}" }
                }
                if is_live {
                    LiveCatalogTable {
                        rows: category_rows,
                        now,
                        can_bid,
                        allow_slow,
                        authenticated,
                        viewer_name: viewer_name.clone(),
                    }
                } else {
                    PreviewCatalogTable { rows: category_rows }
                }
            }
        }
        PublicPager {
            page: page_number(),
            page_size,
            total: Some(page_total),
            has_more: false,
            label: t!("catalog-pagination"),
            on_page_change: move |page| {
                page_number.set(page);
                update_public_query(|query| query.page = page, false);
            },
        }
    }
}

#[component]
fn LiveCatalogTable(
    rows: Vec<CatalogRow>,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("catalog-code")} }
                        th { {t!("catalog-item")} }
                        th { {t!("catalog-level")} }
                        th { {t!("catalog-seller")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-current")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-next-bid")} }
                        th { {t!("catalog-leader")} }
                        th { {t!("catalog-remaining")} }
                        th { {t!("catalog-status")} }
                        th { span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        LiveCatalogRow {
                            row,
                            now,
                            can_bid,
                            allow_slow,
                            authenticated,
                            viewer_name: viewer_name.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PreviewCatalogTable(rows: Vec<CatalogRow>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("catalog-code")} }
                        th { {t!("catalog-item")} }
                        th { {t!("catalog-level")} }
                        th { {t!("catalog-seller")} }
                        th { class: "public-auction-catalog__number", {t!("catalog-start-price")} }
                        th { {t!("catalog-leader")} }
                        th { {t!("catalog-remaining")} }
                        th { {t!("catalog-status")} }
                        th { span { class: "public-visually-hidden", {t!("catalog-actions")} } }
                    }
                }
                tbody {
                    for row in rows {
                        PreviewCatalogRow { row }
                    }
                }
            }
        }
    }
}

#[component]
fn LiveCatalogRow(
    row: CatalogRow,
    now: DateTime<Utc>,
    can_bid: bool,
    allow_slow: bool,
    authenticated: bool,
    viewer_name: Option<String>,
) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let is_leading = viewer_name
        .as_deref()
        .zip(row.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let leading = if is_leading { "public-auction-catalog__leading" } else { "" };
    let countdown = if row.status == "live" {
        countdown_display(&row.remaining, now)
    } else {
        CountdownDisplay { label: t!("value-empty"), urgent: false, expired: false, empty: true }
    };
    let countdown_css_class = countdown_class(&countdown);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let leader = row.leader_name.clone().unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let current_price = format_amount(row.current_price);
    let next_bid = format_amount(row.next_bid);

    rsx! {
        tr { class: "{leading}",
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                a { href: lot_detail_href(&row.lot_id), "{row.code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                span { class: "public-auction-catalog__item-name", title: row.name.clone(), "{row.name}" }
                if let Some(avg_roll) = row.avg_roll {
                    span { class: "public-auction-catalog__roll", "{avg_roll:.1}%" }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td { "data-label": t!("catalog-seller"),
                span { class: "public-auction-catalog__seller", title: seller.clone(), "{seller}" }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-current"),
                "{current_price}"
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-next-bid"),
                "{next_bid}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                if row.leader_name.is_some() {
                    CatalogParticipant {
                        name: leader,
                        marker: None,
                        trailing: is_leading.then(|| t!("catalog-you")),
                    }
                } else {
                    span { class: "public-auction-catalog__none", "{leader}" }
                }
            }
            td {
                class: "{countdown_css_class}",
                "data-label": t!("catalog-remaining"),
                title: if countdown.empty { "".to_owned() } else { row.remaining.clone() },
                "{countdown.label}"
            }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                LiveCatalogActions {
                    lot_id: row.lot_id.clone(),
                    lot_code: row.code.clone(),
                    can_bid: can_bid && row.status == "live",
                    current_price: row.current_price,
                    next_bid: row.next_bid,
                    lot_status: row.status.clone(),
                    allow_slow,
                    authenticated,
                    leading: is_leading,
                    show_view: false,
                    show_proxy_summary: false,
                }
            }
        }
    }
}

#[component]
fn LiveCatalogActions(
    lot_id: String,
    lot_code: String,
    can_bid: bool,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    allow_slow: bool,
    authenticated: bool,
    leading: bool,
    show_view: bool,
    show_proxy_summary: bool,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let requested_lot_id = lot_id.clone();
    let proxy_state = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = requested_lot_id.clone();
        async move { if authenticated { fetch_my_proxy(lot_id).await } else { Ok(None) } }
    });
    let proxy_snapshot = proxy_state.read_unchecked().clone();
    let active_proxy = match proxy_snapshot.as_ref() {
        Some(Ok(Some(proxy))) if proxy.status == "active" => Some(proxy),
        _ => None,
    };
    let proxy_leading =
        active_proxy.and_then(|proxy| proxy.bid.as_ref()).is_some_and(|bid| bid.leading);
    let effective_leading = leading || proxy_leading;
    let ordinary_blocked = effective_leading || active_proxy.is_some();
    let proxy_action_enabled =
        active_proxy.map_or(!effective_leading, |proxy| proxy.capabilities.can_lower);
    let action_reason = if !can_bid {
        t!("bid-unavailable")
    } else if active_proxy.is_some() && !effective_leading {
        t!("proxy-active")
    } else if effective_leading {
        t!("bid-leading")
    } else if !proxy_action_enabled {
        t!("proxy-cannot-lower")
    } else {
        String::new()
    };
    let action_reason_id = format!("public-auction-action-reason-{lot_id}");
    let mut action = use_signal(|| None::<String>);
    let mut proxy_success = use_signal(|| None::<Box<ProxyCommandAuthorization>>);
    let action_name = action();
    let bid_lot_id = lot_id.clone();
    let proxy_lot_id = lot_id.clone();
    let bid_lot_status = lot_status.clone();
    let proxy_lot_status = lot_status.clone();
    let bid_dialog_title = t!("bid-dialog-title", code: lot_code.clone());
    let proxy_dialog_title = t!("proxy-dialog-title", code: lot_code);

    rsx! {
        span { class: "public-auction-catalog__actions-list",
            if show_view {
                a { href: lot_detail_href(&lot_id), {t!("catalog-view")} }
            }
            if authenticated && (can_bid || show_proxy_summary) {
                button {
                    r#type: "button",
                    disabled: !can_bid || ordinary_blocked,
                    aria_describedby: if !can_bid || ordinary_blocked {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        proxy_success.set(None);
                        action.set(if action() == Some("bid".to_owned()) {
                            None
                        } else {
                            Some("bid".to_owned())
                        });
                    },
                    {t!("catalog-bid")}
                }
                button {
                    r#type: "button",
                    disabled: !can_bid || !proxy_action_enabled,
                    aria_describedby: if !can_bid || !proxy_action_enabled {
                        action_reason_id.clone()
                    } else {
                        String::new()
                    },
                    onclick: move |_| {
                        proxy_success.set(None);
                        action.set(if action() == Some("proxy".to_owned()) {
                            None
                        } else {
                            Some("proxy".to_owned())
                        });
                    },
                    if active_proxy.is_some() {
                        {t!("catalog-lower-proxy")}
                    } else {
                        {t!("catalog-proxy")}
                    }
                }
            } else if can_bid {
                Link { to: Route::Login {}, {t!("login-submit")} }
            } else if show_proxy_summary {
                button {
                    r#type: "button",
                    disabled: true,
                    aria_describedby: action_reason_id.clone(),
                    {t!("catalog-bid")}
                }
                button {
                    r#type: "button",
                    disabled: true,
                    aria_describedby: action_reason_id.clone(),
                    {t!("catalog-proxy")}
                }
            }
            if (show_proxy_summary && !can_bid) || ordinary_blocked || !proxy_action_enabled {
                span {
                    id: action_reason_id,
                    class: "public-auction-catalog__leading-reason",
                    role: "note",
                    "{action_reason}"
                }
            }
        }
        if show_proxy_summary && authenticated {
            proxy::ProxySummary {
                snapshot: proxy_snapshot.clone(),
                refresh_generation,
            }
        }
        if action_name == Some("bid".to_owned()) {
            div {
                class: "public-auction-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_label: bid_dialog_title.clone(),
                onclick: move |_| action.set(None),
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        action.set(None);
                    }
                },
                div {
                    class: "public-auction-dialog__panel",
                    onclick: move |event| event.stop_propagation(),
                    div { class: "public-auction-dialog__header",
                        h2 { "{bid_dialog_title}" }
                        button {
                            class: "public-auction-dialog__close",
                            r#type: "button",
                            onclick: move |_| action.set(None),
                            aria_label: t!("catalog-close"),
                            {t!("catalog-close")}
                        }
                    }
                    {commands::bid_form(commands::BidFormInput {
                        lot_id: bid_lot_id,
                        current_price,
                        next_bid,
                        lot_status: bid_lot_status,
                        can_bid,
                        blocked: ordinary_blocked,
                        blocked_reason: action_reason.clone(),
                        on_accepted: EventHandler::new(move |_: ()| action.set(None)),
                        on_close: EventHandler::new(move |_: ()| action.set(None)),
                    })}
                }
            }
        }
        if action_name == Some("proxy".to_owned()) {
            div {
                class: "public-auction-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_label: proxy_dialog_title.clone(),
                onclick: move |_| {
                    proxy_success.set(None);
                    action.set(None);
                },
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        proxy_success.set(None);
                        action.set(None);
                    }
                },
                div {
                    class: "public-auction-dialog__panel",
                    onclick: move |event| event.stop_propagation(),
                    div { class: "public-auction-dialog__header",
                        h2 { "{proxy_dialog_title}" }
                        button {
                            class: "public-auction-dialog__close",
                            r#type: "button",
                            onclick: move |_| {
                                proxy_success.set(None);
                                action.set(None);
                            },
                            aria_label: t!("catalog-close"),
                            {t!("catalog-close")}
                        }
                    }
                    {commands::proxy_form(commands::ProxyFormInput {
                        lot_id: proxy_lot_id,
                        current_price,
                        next_bid,
                        lot_status: proxy_lot_status,
                        allow_slow,
                        leading: effective_leading,
                        blocked_reason: action_reason,
                        proxy: proxy_snapshot,
                        refresh_generation,
                        saved_authorization: proxy_success,
                        on_close: EventHandler::new(move |_: ()| {
                            proxy_success.set(None);
                            action.set(None);
                        }),
                    })}
                }
            }
        }
    }
}

/// 公开拍卖参与者标识；长名称保持可读的省略样式，并保留旧版的代理/当前用户标记。
#[component]
fn CatalogParticipant(name: String, marker: Option<String>, trailing: Option<String>) -> Element {
    rsx! {
        span { class: "public-auction-participant-identity",
            if let Some(marker) = marker {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--leading",
                    "{marker}"
                }
            }
            span {
                class: "public-auction-participant-identity__name",
                title: name.clone(),
                "{name}"
            }
            if let Some(trailing) = trailing {
                span {
                    class: "public-auction-participant-identity__marker public-auction-participant-identity__marker--trailing",
                    "{trailing}"
                }
            }
        }
    }
}

#[component]
fn PreviewCatalogRow(row: CatalogRow) -> Element {
    let status_label = lot_status_label(&row.status);
    let status_class = lot_status_class(&row.status);
    let level = row.level.map(|value| value.to_string()).unwrap_or_else(|| t!("value-empty"));
    let seller = row.seller_name.clone();
    let start_price = format_amount(row.start_price);

    rsx! {
        tr {
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                a { href: lot_detail_href(&row.lot_id), "{row.code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"),
                span { class: "public-auction-catalog__item-name", title: row.name.clone(), "{row.name}" }
                if let Some(avg_roll) = row.avg_roll {
                    span { class: "public-auction-catalog__roll", "{avg_roll:.1}%" }
                }
            }
            td { "data-label": t!("catalog-level"), "{level}" }
            td { "data-label": t!("catalog-seller"),
                span { class: "public-auction-catalog__seller", title: seller.clone(), "{seller}" }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-start-price"),
                "{start_price}"
            }
            td { class: "public-auction-catalog__participant-cell", "data-label": t!("catalog-leader"),
                span { class: "public-auction-catalog__none", {t!("value-empty")} }
            }
            td { class: "public-auction-countdown--none", "data-label": t!("catalog-remaining"), {t!("value-empty")} }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status_class}", "{status_label}" }
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                span { class: "public-auction-catalog__actions-list",
                    a { href: lot_detail_href(&row.lot_id), {t!("catalog-view")} }
                }
            }
        }
    }
}

fn grouped_catalog_rows(rows: &[CatalogRow], active: &str) -> Vec<(&'static str, Vec<CatalogRow>)> {
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

fn catalog_count(counts: &BTreeMap<String, i64>, category: &str, rows: &[CatalogRow]) -> i64 {
    counts.get(category).copied().unwrap_or_else(|| {
        if category == "all" {
            rows.len() as i64
        } else {
            rows.iter().filter(|row| catalog_category_key(&row.category) == category).count() as i64
        }
    })
}

fn catalog_category_key(category: &str) -> &'static str {
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

fn catalog_category_code(category: &str) -> &'static str {
    CATALOG_CATEGORIES
        .iter()
        .find_map(|(key, code)| (*key == category).then_some(*code))
        .unwrap_or("Other")
}

fn catalog_category_label(category: &str) -> String {
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

fn lot_status_label(status: &str) -> String {
    match status {
        "pending" => t!("status-pending"),
        "live" => t!("status-live"),
        "sold" => t!("status-sold"),
        "unsold" => t!("status-unsold"),
        _ => status.to_owned(),
    }
}

fn lot_status_class(status: &str) -> &'static str {
    match status {
        "pending" => "pending",
        "live" => "live",
        "sold" => "sold",
        "unsold" => "unsold",
        _ => "unknown",
    }
}

fn auction_status_label(status: &str) -> String {
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

fn auction_status_class(status: &str) -> &'static str {
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

fn auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

fn historical_auction_href(auction_id: &str, world_id: Option<&str>) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = world_id.and_then(normalize_world_query);
    public_href("/auction", &query)
}

fn historical_history_href(auction_id: &str, world_id: &str) -> String {
    let mut query = read_public_query();
    query.auction_id = Some(auction_id.to_owned());
    query.world = normalize_world_query(world_id);
    query.category = None;
    query.status = None;
    query.page = 1;
    public_href("/auction", &query)
}

fn lot_detail_href(lot_code: &str) -> String {
    public_href(&format!("/lots/{lot_code}"), &read_public_query())
}

fn current_auction_href() -> String {
    public_href("/auction", &read_public_query())
}

fn format_auction_time(value: &str) -> String {
    format_localized_time(value)
}

/// 解析旧版 Public 使用的 credit 缩写；无后缀和 `k` 表示千，`m` 支持最多两位小数。
///
/// 浏览器只用该结果做即时提示，服务端仍会再次校验金额、拍卖状态和竞价资格。
fn parse_credit_amount(input: &str) -> Option<i64> {
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

struct CountdownDisplay {
    label: String,
    urgent: bool,
    expired: bool,
    empty: bool,
}

fn countdown_display(ends_at: &str, now: DateTime<Utc>) -> CountdownDisplay {
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
        label: if days > 0 { format!("{days}d {clock}") } else { clock },
        urgent: remaining_seconds <= 15 * 60,
        expired: false,
        empty: false,
    }
}

fn countdown_class(countdown: &CountdownDisplay) -> &'static str {
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
fn Placeholder(title: String, detail: String) -> Element {
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

#[component]
fn AuctionList() -> Element {
    let query = read_public_query();
    let world_filter = use_signal(|| query.world.clone().unwrap_or_else(|| "all".to_owned()));
    let status_filter = use_signal(|| query.status.clone().unwrap_or_else(|| "settled".to_owned()));
    let page_number = use_signal(|| query.page.max(1));
    let refresh_generation = use_signal(|| 0_u64);
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let auctions = use_resource(move || {
        let _ = refresh_generation();
        let world = world_filter();
        let status = status_filter();
        let page = page_number();
        let world = (world != "all").then_some(world);
        fetch_auction_page(world, status, page, 20)
    });
    match &*auctions.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            AuctionHistoryPage {
                page: page.clone(),
                world_filter,
                status_filter,
                page_number,
                world_options,
            }
        },
    }
}

/// 公开端错误状态；重试只重新触发当前资源读取，不在浏览器端伪造数据。
#[component]
fn RetryPlaceholder(title: String, detail: String, mut refresh_generation: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "public-auction-empty",
            role: "alert",
            aria_labelledby: "retry-placeholder-title",
            h1 { id: "retry-placeholder-title", "{title}" }
            p { "{detail}" }
            button {
                r#type: "button",
                onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                {t!("data-retry")}
            }
        }
    }
}

#[component]
fn AuctionHistoryPage(
    page: AuctionListPage,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
    world_options: Vec<String>,
) -> Element {
    let total = page.total.unwrap_or(page.auctions.len() as i64);
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-history-title",
            h1 { id: "auction-history-title", class: "public-auction-category__title",
                {t!("page-auctions-title")}
            }
            p { class: "public-auction-page__status", {t!("page-auctions-note")} }
            div { class: "public-auction-history-controls",
                label {
                    {t!("auction-history-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            let world = event.value();
                            world_filter.set(world.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.world = (world != "all").then_some(world.clone());
                                    query.status = Some(status_filter());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        option { value: "all", {t!("auction-history-world-all")} }
                        for world in world_options {
                            option { value: world.clone(), {world_label(&world)} }
                        }
                    }
                }
                label {
                    {t!("auction-history-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            let status = event.value();
                            status_filter.set(status.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.status = Some(status.clone());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        option { value: "settled", {t!("auction-history-status-settled")} }
                        option { value: "final", {t!("auction-history-status-final")} }
                        option { value: "live", {t!("auction-history-status-live")} }
                    }
                }
            }
            if page.auctions.is_empty() {
                Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
            } else {
                AuctionHistoryTable { rows: page.auctions.clone() }
            }
            PublicPager {
                page: page_number(),
                page_size,
                total: Some(total),
                has_more: false,
                label: t!("catalog-pagination"),
                on_page_change: move |page| {
                    page_number.set(page);
                    update_public_query(|query| query.page = page, false);
                },
            }
        }
    }
}

#[component]
fn AuctionHistoryTable(rows: Vec<HistoricalAuction>) -> Element {
    rsx! {
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("auction-history-number")} }
                        th { {t!("auction-history-world")} }
                        th { {t!("auction-history-ended")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-lots")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-sold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-unsold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-total")} }
                        th { {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for auction in rows {
                        {
                            let ended = auction.settled_at.clone()
                                .or_else(|| auction.effective_end_at.clone())
                                .or_else(|| auction.opened_at.clone())
                                .unwrap_or_else(|| auction.scheduled_open_at.clone());
                            let href = historical_history_href(&auction.id, &auction.world_id);
                            rsx! {
                                tr {
                                    td { class: "public-auction-catalog__number", "#{auction.sequence}" }
                                    td { "{auction_world_label(&auction.world_id)}" }
                                    td { time { datetime: ended.clone(), "{format_auction_time(&ended)}" } }
                                    td { class: "public-auction-catalog__number", "{auction.lot_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.sold_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.unsold_count}" }
                                    td { class: "public-auction-catalog__amount public-auction-catalog__number", "{format_amount(auction.sold_amount)}" }
                                    td {
                                        a { href, {t!("catalog-view")} }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn LotDetail(lot_code: String) -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let _fallback_poll = use_future(move || async move {
        loop {
            TimeoutFuture::new(5_000).await;
            if realtime_state() != LiveConnectionState::Connected {
                refresh_generation.set(refresh_generation().saturating_add(1));
            }
        }
    });
    let lot_id = lot_code.clone();
    let detail = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = lot_id.clone();
        async move { fetch_lot_detail(lot_id).await }
    });

    match &*detail.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-lot-title", code: lot_code.clone()), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-lot-title", code: lot_code.clone()),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                PreviewLotDetailView { lot: lot.clone() }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LiveLotDetailView {
                    auction_id: page.auction_id.clone(),
                    auction_status: page.auction_status.clone(),
                    bidding_open: page.bidding_open,
                    lot: lot.clone(),
                }
            },
            None => rsx! {
                Placeholder { title: t!("page-lot-title", code: lot_code), detail: t!("data-empty") }
            },
        },
    }
}

#[component]
fn PreviewLotDetailView(lot: crate::api::PreviewLotDetail) -> Element {
    let back_href = current_auction_href();
    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot.summary.code.clone(),
                seller: lot.summary.seller_name.clone(),
                status: lot.summary.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard { equipment: lot.equipment.clone() }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("catalog-start-price")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.summary.start_price)}" }
                    }
                    p { class: "public-auction-page__status", {t!("lot-not-live")} }
                }
            }
        }
    }
}

#[component]
fn LiveLotDetailView(
    auction_id: String,
    auction_status: String,
    bidding_open: bool,
    lot: crate::api::LiveLot,
) -> Element {
    let back_href = current_auction_href();
    let mut clock = use_signal(estimated_now);
    let _clock_task = use_future(move || async move {
        loop {
            TimeoutFuture::new(1_000).await;
            clock.set(estimated_now());
        }
    });
    let mut refresh_generation = use_context::<Signal<u64>>();
    let session = use_resource(fetch_public_session);
    let viewer_name = match &*session.read_unchecked() {
        Some(Ok(Some(session))) => Some(session.username.clone()),
        _ => None,
    };
    let authenticated = matches!(&*session.read_unchecked(), Some(Ok(Some(_))));
    let is_leading = viewer_name
        .as_deref()
        .zip(lot.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let mut bid_page = use_signal(|| 1_i64);
    let bid_log = use_resource({
        let auction_id = auction_id.clone();
        let lot_id = lot.id.clone();
        move || {
            let _ = refresh_generation();
            let page = bid_page();
            fetch_lot_bid_log(auction_id.clone(), lot_id.clone(), page)
        }
    });
    let countdown =
        lot.effective_end_at.as_deref().map(|ends_at| countdown_display(ends_at, clock()));

    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            p {
                aria_live: "polite",
                class: "public-visually-hidden",
                {t!("auction-realtime-status")}
            }
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot_code_for(&lot.equipment.category, lot.lot_number),
                seller: lot.seller_name.clone(),
                status: lot.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard { equipment: lot.equipment.clone() }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("lot-current-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.current_price)}" }
                        dt { {t!("lot-next-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.next_bid)}" }
                        dt { {t!("lot-leader")} }
                        dd {
                            if let Some(leader) = lot.leader_name.clone() {
                                CatalogParticipant {
                                    name: leader,
                                    marker: None,
                                    trailing: is_leading.then(|| t!("catalog-you")),
                                }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                        dt { {t!("lot-ends-in")} }
                        dd {
                            if let (Some(ends_at), Some(countdown)) =
                                (lot.effective_end_at.clone(), countdown)
                            {
                                time { datetime: ends_at, class: countdown_class(&countdown), "{countdown.label}" }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                    }
                    LiveCatalogActions {
                        lot_id: lot.id.clone(),
                        lot_code: lot_code_for(&lot.equipment.category, lot.lot_number),
                        can_bid: bidding_open && lot.status == "live",
                        current_price: lot.current_price,
                        next_bid: lot.next_bid,
                        lot_status: lot.status.clone(),
                        allow_slow: auction_status == "live",
                        authenticated,
                        leading: is_leading,
                        show_view: false,
                        show_proxy_summary: true,
                    }
                }
            }
            section { class: "public-auction-lot__history", aria_labelledby: "lot-bid-history-title",
                h2 { id: "lot-bid-history-title", {t!("lot-bid-history")} }
                match &*bid_log.read_unchecked() {
                    None => rsx! { p { role: "status", {t!("data-loading")} } },
                    Some(Err(_)) => rsx! {
                        div { class: "public-auction-page__status", role: "alert",
                            p { {t!("data-error")} }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    refresh_generation
                                        .set(refresh_generation().saturating_add(1));
                                },
                                {t!("data-retry")}
                            }
                        }
                    },
                    Some(Ok(page)) if page.bids.is_empty() => rsx! {
                        p { class: "public-auction-lot__none", {t!("lot-no-bids")} }
                    },
                    Some(Ok(page)) => rsx! {
                        BidHistoryTable { page: page.clone() }
                        PublicPager {
                            page: bid_page(),
                            page_size: page.page_size,
                            total: page.total,
                            has_more: page.has_more.unwrap_or(false),
                            label: t!("lot-bid-history"),
                            on_page_change: move |next_page| bid_page.set(next_page),
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn LotDetailBack(href: String) -> Element {
    rsx! {
        p { class: "public-auction-lot__back",
            a { href, "← ", {t!("lot-back-to-auction")} }
        }
    }
}

#[component]
fn LotDetailHeader(code: String, seller: String, status: String) -> Element {
    rsx! {
        header { class: "public-auction-lot__header",
            h1 { id: "lot-detail-title", "{code}" }
            span { class: "public-auction-status public-auction-status--{lot_status_class(&status)}", "{lot_status_label(&status)}" }
            span { class: "public-auction-lot__seller",
                {t!("catalog-seller")}, ": ", "{seller}"
            }
        }
    }
}

#[component]
fn EquipmentCard(equipment: EquipmentDetail) -> Element {
    let mut stats_page = use_signal(|| 1_i64);
    let stats_page_size = 10_i64;
    let current_stats_page = stats_page().max(1);
    let stats_total = i64::try_from(equipment.stats.len()).unwrap_or(i64::MAX);
    let stats_start =
        usize::try_from(current_stats_page.saturating_sub(1).saturating_mul(stats_page_size))
            .unwrap_or(usize::MAX);
    rsx! {
        div { class: "public-auction-equipment",
            h2 { "{equipment.name}" }
            dl {
                dt { {t!("equipment-category")} }
                dd { {catalog_category_label(catalog_category_key(&equipment.category))} }
                if let Some(level) = equipment.level {
                    dt { {t!("equipment-level")} }
                    dd { "{level}" }
                }
            }
            if !equipment.info_summary.is_empty() {
                p { class: "public-auction-item-summary", "{equipment.info_summary}" }
            }
            if let Some(avg_roll) = equipment.avg_roll {
                p { class: "public-auction-equipment__avg-roll",
                    span { {t!("equipment-avg-roll")} }
                    strong { "{avg_roll:.1}%" }
                }
            }
            if !equipment.stats.is_empty() {
                div { class: "public-auction-equipment__stats-wrap",
                    table { class: "public-auction-equipment__stats",
                        caption { {t!("equipment-stats")} }
                        thead {
                            tr {
                                th { {t!("equipment-stat-name")} }
                                th { {t!("equipment-stat-value")} }
                                th { {t!("equipment-stat-base")} }
                                th { {t!("equipment-stat-roll")} }
                            }
                        }
                        tbody {
                            for (name, stat) in equipment.stats.iter().skip(stats_start).take(stats_page_size as usize) {
                                tr {
                                    th { scope: "row", "{name}" }
                                    td { "{stat.value}" }
                                    td { {stat.base.map(|value| format!("{value:.2}")).unwrap_or_else(|| t!("value-empty"))} }
                                    td { {stat.roll.map(|value| format!("{value:.1}%")).unwrap_or_else(|| t!("value-empty"))} }
                                }
                            }
                        }
                    }
                }
                PublicPager {
                    page: current_stats_page,
                    page_size: stats_page_size,
                    total: Some(stats_total),
                    has_more: false,
                    label: t!("equipment-stats"),
                    on_page_change: move |next_page| stats_page.set(next_page),
                }
            }
            if !equipment.game_equipment_url.is_empty() {
                a {
                    class: "public-auction-equipment__link",
                    href: equipment.game_equipment_url,
                    target: "_blank",
                    rel: "noreferrer",
                    {t!("equipment-view-in-game")}
                }
            }
        }
    }
}

#[component]
fn BidHistoryTable(page: crate::api::BidLogPage) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table {
                thead {
                    tr {
                        th { {t!("bidlog-bidder")} }
                        th { class: "public-auction-lot__number", {t!("bidlog-amount")} }
                        th { {t!("bidlog-mechanism")} }
                        th { {t!("bidlog-time")} }
                    }
                }
                tbody {
                    for bid in page.bids {
                        tr { class: if bid.valid { "public-auction-lot__leading" } else { "" },
                            td { class: "public-auction-lot__participant-cell", "data-label": t!("bidlog-bidder"),
                                CatalogParticipant {
                                    name: bid.bidder_name.clone(),
                                    marker: (bid.mechanism == "proxy").then(|| t!("catalog-proxy-marker")),
                                    trailing: None,
                                }
                            }
                            td { class: "public-auction-lot__amount public-auction-lot__number", "data-label": t!("bidlog-amount"), "{format_amount(bid.amount)}" }
                            td { "data-label": t!("bidlog-mechanism"), "{bid_mechanism_label(&bid.mechanism)}" }
                            td { "data-label": t!("bidlog-time"), time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" } }
                        }
                    }
                }
            }
        }
    }
}

fn bid_mechanism_label(mechanism: &str) -> String {
    match mechanism {
        "proxy" => t!("bidlog-mechanism-proxy"),
        _ => t!("bidlog-mechanism-ordinary"),
    }
}

#[component]
fn BidLog() -> Element {
    reads::bid_log()
}

#[component]
fn History() -> Element {
    reads::history()
}

#[component]
fn Login() -> Element {
    session::login()
}

#[component]
fn Me() -> Element {
    account::page()
}

#[component]
fn Account() -> Element {
    redirect_public_alias("/me")
}

#[component]
fn Rules() -> Element {
    rules::page()
}

#[component]
fn Blacklist() -> Element {
    blacklist::page()
}

#[component]
fn CompensationPool() -> Element {
    compensation::pool_page()
}

#[component]
fn Compensation() -> Element {
    redirect_public_alias("/compensation-pool")
}

#[component]
fn Status() -> Element {
    realtime::page()
}

/// 未匹配任何已冻结路由时回到公开端首页。
///
/// 旧版 Public 对未知路径使用无历史记录跳转，而不是在外壳之外渲染独立 404；复用别名
/// 跳转边界也会保留旧 URL 中的筛选查询，避免分享链接意外丢失上下文。
#[component]
fn NotFound(segments: Vec<String>) -> Element {
    let _ = segments;
    redirect_public_alias("/")
}
