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
mod reads;
mod realtime;
mod rules;
mod session;

use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use unic_langid::langid;

use crate::api::{
    CatalogRow, CurrentAuction, HistoricalAuction, LotDetailItem, RealtimeEvent,
    fetch_auction_list, fetch_current_auction, fetch_current_lots, fetch_lot_detail,
    subscribe_realtime,
};
use auction_web_shared::{
    BaseStylesheet, LanguagePicker,
    i18n::{DEFAULT_PUBLIC_LOCALE, initial_language},
    use_locale_effects,
};

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css");

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

/// 公开端外壳：导航与主区域。Admin 不复用该外壳。
#[component]
fn PublicShell() -> Element {
    rsx! {
        main { class: "auction-shell",
            nav { aria_label: t!("nav-label"),
                Link { to: Route::AuctionIndex {}, {t!("nav-auctions")} }
                Link { to: Route::Account {}, {t!("nav-my-bids")} }
                Link { to: Route::Status {}, {t!("nav-status")} }
                LanguagePicker { label: t!("language-label") }
            }
            Outlet::<Route> {}
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
    rsx! {
        AuctionBoard {}
    }
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不再渲染迁移阶段示例数据。
#[component]
fn AuctionBoard() -> Element {
    let refresh_generation = use_signal(|| 0_u64);
    let current = use_resource(move || {
        let _ = refresh_generation();
        fetch_current_auction()
    });
    let _realtime = use_future({
        let mut refresh_generation = refresh_generation;
        move || async move {
            let _ = subscribe_realtime(0, move |page| {
                if page.events.iter().any(should_refresh_auction_board) {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                }
            })
            .await;
        }
    });

    match &*current.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-error") }
        },
        Some(Ok(None)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(auction))) => rsx! {
            AuctionData { auction: auction.clone() }
        },
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

#[component]
fn AuctionData(auction: CurrentAuction) -> Element {
    let auction_id = auction.id.clone();
    let lots = use_resource(move || {
        let auction_id = auction_id.clone();
        async move { fetch_current_lots(auction_id).await }
    });
    let status_label = auction_status_label(&auction.status);
    let catalog = match &*lots.read_unchecked() {
        None => {
            rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-loading")} } }
        }
        Some(Err(_)) => {
            rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-error")} } }
        }
        Some(Ok(page)) => {
            let rows = page.rows();
            rsx! { CatalogTable { rows } }
        }
    };
    let lot_count = match &*lots.read_unchecked() {
        Some(Ok(page)) => page.rows().len().to_string(),
        _ => "—".to_owned(),
    };

    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            header { class: "public-auction-round",
                div { class: "public-auction-round__title",
                    h1 { id: "auction-title", {t!("auction-title")} }
                    span { class: "public-auction-round__world", "{auction.world}" }
                    span { class: "public-auction-round__status", "{status_label}" }
                }
                p { class: "public-auction-round__end",
                    {t!("auction-ends-at")}
                    time { datetime: auction.scheduled_ends_at.clone().unwrap_or_default(),
                        {auction.scheduled_ends_at.clone().unwrap_or_else(|| "—".to_owned())}
                    }
                }
            }

            div { class: "public-auction-summary",
                div {
                    p { class: "public-auction-summary__eyebrow", {t!("auction-round-eyebrow")} }
                    h2 { "#{auction.number}" }
                    p { "{auction.presentation_mode}" }
                }
                dl { class: "public-auction-preview-stats",
                    div {
                        dt { {t!("stats-lots")} }
                        dd { "{lot_count}" }
                    }
                    div {
                        dt { {t!("stats-bids")} }
                        dd { "{status_label}" }
                    }
                }
            }

            {catalog}
            {forum::panel(auction.id.clone())}
        }
    }
}

#[component]
fn CatalogTable(rows: Vec<CatalogRow>) -> Element {
    rsx! {
        section { class: "public-auction-category",
            h2 { class: "public-auction-category__title", {t!("catalog-item")} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("catalog-code")} }
                            th { {t!("catalog-item")} }
                            th { {t!("catalog-status")} }
                            th { class: "public-auction-catalog__number", {t!("catalog-price")} }
                            th { {t!("catalog-remaining")} }
                            th { {t!("catalog-actions")} }
                        }
                    }
                    tbody {
                        for row in rows {
                            AuctionRow { row }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AuctionRow(row: CatalogRow) -> Element {
    let status_label = match row.status.as_str() {
        "live" => t!("status-live"),
        "sold" => t!("status-sold"),
        _ => t!("status-ended"),
    };
    let leading = if row.status == "live" { "public-auction-catalog__leading" } else { "" };
    // 已结束的拍品没有倒计时，显示本地化的结束标记而不是空白单元格。
    let remaining =
        if row.remaining.is_empty() { t!("status-ended") } else { row.remaining.clone() };

    rsx! {
        tr { class: "{leading}",
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                Link { to: Route::LotDetail { lot_code: row.lot_id.clone() }, "{row.code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"), "{row.name}" }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{row.status}",
                    "{status_label}"
                }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-price"),
                "{row.price}"
            }
            td {
                class: "public-auction-countdown",
                "data-label": t!("catalog-remaining"),
                "{remaining}"
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                span { class: "public-auction-catalog__actions-list",
                    Link { to: Route::LotDetail { lot_code: row.lot_id.clone() }, {t!("catalog-view")} }
                    button { r#type: "button", disabled: true, {t!("catalog-bid")} }
                }
            }
        }
    }
}

fn auction_status_label(status: &str) -> String {
    match status {
        "live" => t!("auction-status-live"),
        "sold" | "settled" | "final" => t!("status-ended"),
        _ => status.to_owned(),
    }
}

/// 迁移阶段的占位页。
///
/// 各页面在对应能力完成契约、用例和持久化边界后替换为真实实现。
#[component]
fn Placeholder(title: String, detail: String) -> Element {
    rsx! {
        section { class: "public-auction-empty", aria_labelledby: "placeholder-title",
            h1 { id: "placeholder-title", "{title}" }
            p { "{detail}" }
        }
    }
}

#[component]
fn AuctionList() -> Element {
    let auctions = use_resource(fetch_auction_list);
    match &*auctions.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-error") }
        },
        Some(Ok(rows)) if rows.is_empty() => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
        },
        Some(Ok(rows)) => rsx! {
            AuctionHistoryTable { rows: rows.clone() }
        },
    }
}

#[component]
fn AuctionHistoryTable(rows: Vec<HistoricalAuction>) -> Element {
    rsx! {
        section { class: "public-auction-category",
            h1 { class: "public-auction-category__title", {t!("page-auctions-title")} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("catalog-code")} }
                            th { {t!("catalog-status")} }
                            th { {t!("catalog-item")} }
                            th { class: "public-auction-catalog__number", {t!("catalog-price")} }
                            th { {t!("catalog-actions")} }
                        }
                    }
                    tbody {
                        for auction in rows {
                            tr {
                                td { "{auction.world_id}-{auction.sequence}" }
                                td { "{auction_status_label(&auction.status)}" }
                                td { "{auction.lot_count}" }
                                td { class: "public-auction-catalog__number", "{auction.sold_amount}" }
                                td {
                                    Link { to: Route::AuctionPage {}, {t!("catalog-view")} }
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
    let lot_id = lot_code.clone();
    let detail = use_resource(move || {
        let lot_id = lot_id.clone();
        async move { fetch_lot_detail(lot_id).await }
    });

    match &*detail.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-lot-title", code: lot_code.clone()), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-lot-title", code: lot_code.clone()), detail: t!("data-error") }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                LotDetailPanel {
                    lot_id: None,
                    title: lot.equipment_name.clone(),
                    status: lot.status.clone(),
                    price: lot.start_price,
                    seller: lot.seller_name.clone(),
                }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LotDetailPanel {
                    lot_id: Some(lot.id.clone()),
                    title: lot.equipment.name.clone(),
                    status: lot.status.clone(),
                    price: lot.current_price,
                    seller: t!("value-empty"),
                }
            },
            None => rsx! {
                Placeholder { title: t!("page-lot-title", code: lot_code), detail: t!("data-empty") }
            },
        },
    }
}

#[component]
fn LotDetailPanel(
    lot_id: Option<String>,
    title: String,
    status: String,
    price: i64,
    seller: String,
) -> Element {
    rsx! {
        section { class: "public-auction-empty", aria_labelledby: "lot-detail-title",
            h1 { id: "lot-detail-title", "{title}" }
            p { span { {t!("catalog-status")} } span { ": " } "{status}" }
            p { span { {t!("catalog-price")} } span { ": " } "{price}" }
            p { span { {t!("catalog-item")} } span { ": " } "{seller}" }
            if let Some(lot_id) = lot_id {
                {commands::bid_form(lot_id.clone())}
                {commands::proxy_form(lot_id)}
            }
        }
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
    session::account()
}

#[component]
fn Account() -> Element {
    account::page()
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
    compensation::movement_page()
}

#[component]
fn Status() -> Element {
    realtime::page()
}

/// 未匹配任何已冻结路由时的页面。
///
/// 该组件在 `PublicShell` 之外，因此自行渲染外壳容器。
#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        main { class: "auction-shell",
            section { class: "public-auction-empty",
                h1 { {t!("not-found-title")} }
                p { {t!("not-found-note")} }
            }
        }
    }
}
