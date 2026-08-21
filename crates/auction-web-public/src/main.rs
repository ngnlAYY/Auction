//! 公开端浏览器界面（Dioxus CSR）。
//!
//! 路由集合按旧版 Auction-BF 的 `manifests/public.routes.json` 冻结，保持 URL 契约不变。
//! 界面文案全部经由 `dioxus-i18n` 取自 `i18n/*.ftl`，默认英文；受支持语言与持久化行为
//! 由 `auction-web-shared` 统一提供。
//!
//! 本 crate 只允许依赖 `auction-contracts` 的稳定 DTO 与 `auction-web-shared` 的基础能力，
//! 不得导入存储、配置或凭据访问路径。

use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use unic_langid::langid;

use auction_web_shared::{
    BaseStylesheet, LanguagePicker,
    i18n::{DEFAULT_PUBLIC_LOCALE, initial_language},
    use_locale_effects,
};

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css");

fn main() {
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
            .with_locale((langid!("en"), include_str!("../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../i18n/ko.ftl")))
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

/// 拍卖看板。
///
/// 当前为迁移阶段的静态展示结构，沿用旧版公共拍卖页的 BEM 类名与视觉契约；
/// 接入真实数据前需要先完成契约冻结、应用用例与持久化边界。
#[component]
fn AuctionBoard() -> Element {
    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            header { class: "public-auction-round",
                div { class: "public-auction-round__title",
                    h1 { id: "auction-title", {t!("auction-title")} }
                    span { class: "public-auction-round__world", {t!("auction-world-main")} }
                    span { class: "public-auction-round__status public-auction-round__status--live",
                        {t!("auction-status-live")}
                    }
                }
                p { class: "public-auction-round__end",
                    {t!("auction-ends-at")}
                    span { class: "public-auction-countdown", "02:14:36" }
                    time { datetime: "2026-08-21T22:00:00Z", "2026-08-21 22:00 UTC" }
                }
            }

            div { class: "public-auction-summary",
                div {
                    p { class: "public-auction-summary__eyebrow", {t!("auction-round-eyebrow")} }
                    h2 { {t!("auction-round-title")} }
                    p { {t!("auction-round-note")} }
                }
                dl { class: "public-auction-preview-stats",
                    div {
                        dt { {t!("stats-lots")} }
                        dd { "12" }
                    }
                    div {
                        dt { {t!("stats-bids")} }
                        dd { "8" }
                    }
                }
            }

            nav { class: "public-auction-filter", aria_label: t!("filter-label"),
                button { class: "public-auction-filter__tab", r#type: "button", aria_pressed: "true",
                    {t!("filter-all")}
                    span { class: "public-auction-filter__count", "12" }
                }
                button { class: "public-auction-filter__tab", r#type: "button", aria_pressed: "false",
                    {t!("filter-one-handed")}
                    span { class: "public-auction-filter__count", "4" }
                }
                button { class: "public-auction-filter__tab", r#type: "button", aria_pressed: "false",
                    {t!("filter-two-handed")}
                    span { class: "public-auction-filter__count", "3" }
                }
                button { class: "public-auction-filter__tab", r#type: "button", aria_pressed: "false",
                    {t!("filter-other")}
                    span { class: "public-auction-filter__count", "5" }
                }
            }

            section { class: "public-auction-category",
                h2 { class: "public-auction-category__title",
                    {t!("category-one-handed")}
                    span { class: "public-auction-category__code", {t!("category-code-one")} }
                }
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
                            AuctionRow {
                                code: "one-001",
                                name: "Legendary Rapier",
                                price: "12,000",
                                status: "live",
                                time: "01:24:16",
                            }
                            AuctionRow {
                                code: "one-002",
                                name: "Fine Wakizashi",
                                price: "8,500",
                                status: "live",
                                time: "02:04:42",
                            }
                            AuctionRow {
                                code: "one-003",
                                name: "Peerless Shortsword",
                                price: "6,250",
                                status: "sold",
                                time: "",
                            }
                        }
                    }
                }
            }

            p { class: "public-auction-page__status", role: "status", {t!("migration-note")} }
        }
    }
}

#[component]
fn AuctionRow(
    code: &'static str,
    name: &'static str,
    price: &'static str,
    status: &'static str,
    time: &'static str,
) -> Element {
    let status_label = if status == "sold" { t!("status-sold") } else { t!("status-live") };
    let leading = if status == "live" { "public-auction-catalog__leading" } else { "" };
    // 已结束的拍品没有倒计时，显示本地化的结束标记而不是空白单元格。
    let remaining = if time.is_empty() { t!("status-ended") } else { time.to_owned() };

    rsx! {
        tr { class: "{leading}",
            td { class: "public-auction-catalog__code", "data-label": t!("catalog-code"),
                Link { to: Route::LotDetail { lot_code: code.to_owned() }, "{code}" }
            }
            td { class: "public-auction-catalog__name", "data-label": t!("catalog-item"), "{name}" }
            td { "data-label": t!("catalog-status"),
                span { class: "public-auction-status public-auction-status--{status}",
                    "{status_label}"
                }
            }
            td {
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "data-label": t!("catalog-price"),
                "{price}"
            }
            td {
                class: "public-auction-countdown",
                "data-label": t!("catalog-remaining"),
                "{remaining}"
            }
            td { class: "public-auction-catalog__actions", "data-label": t!("catalog-actions"),
                span { class: "public-auction-catalog__actions-list",
                    Link { to: Route::LotDetail { lot_code: code.to_owned() }, {t!("catalog-view")} }
                    button { r#type: "button", disabled: status == "sold", {t!("catalog-bid")} }
                }
            }
        }
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
    rsx! {
        Placeholder { title: t!("page-auctions-title"), detail: t!("page-auctions-note") }
    }
}

#[component]
fn LotDetail(lot_code: String) -> Element {
    rsx! {
        Placeholder {
            title: t!("page-lot-title", code: lot_code.clone()),
            detail: t!("page-lot-note"),
        }
    }
}

#[component]
fn BidLog() -> Element {
    rsx! {
        Placeholder { title: t!("page-bidlog-title"), detail: t!("page-bidlog-note") }
    }
}

#[component]
fn History() -> Element {
    rsx! {
        Placeholder { title: t!("page-history-title"), detail: t!("page-history-note") }
    }
}

#[component]
fn Login() -> Element {
    rsx! {
        Placeholder { title: t!("page-login-title"), detail: t!("page-login-note") }
    }
}

#[component]
fn Me() -> Element {
    rsx! {
        Placeholder { title: t!("page-me-title"), detail: t!("page-me-note") }
    }
}

#[component]
fn Account() -> Element {
    rsx! {
        Placeholder { title: t!("page-account-title"), detail: t!("page-account-note") }
    }
}

#[component]
fn Rules() -> Element {
    rsx! {
        Placeholder { title: t!("page-rules-title"), detail: t!("page-rules-note") }
    }
}

#[component]
fn Blacklist() -> Element {
    rsx! {
        Placeholder { title: t!("page-blacklist-title"), detail: t!("page-blacklist-note") }
    }
}

#[component]
fn CompensationPool() -> Element {
    rsx! {
        Placeholder {
            title: t!("page-compensation-pool-title"),
            detail: t!("page-compensation-pool-note"),
        }
    }
}

#[component]
fn Compensation() -> Element {
    rsx! {
        Placeholder {
            title: t!("page-compensation-title"),
            detail: t!("page-compensation-note"),
        }
    }
}

#[component]
fn Status() -> Element {
    rsx! {
        Placeholder { title: t!("page-status-title"), detail: t!("page-status-note") }
    }
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
