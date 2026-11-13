//! 公开端控制台外壳：顶部 ticker、可收起分组侧栏与倒计时叶子。
//! 页面内容、查询与实时状态仍由既有页面组件拥有；本模块只做展示与导航。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::surface::{Route, use_countdown_display};

/// 顶部 ticker 的剩余时间；秒级节拍只订阅在这个叶子组件。
#[component]
pub(crate) fn ConsoleCountdown(ends_at: String) -> Element {
    let countdown = use_countdown_display(ends_at);
    rsx! {
        span {
            class: "console-ticker__countdown",
            aria_label: t!("auction-countdown"),
            "{countdown.label}"
        }
    }
}

/// 侧栏图标；统一 18px、1.5 stroke，只表达导航语义。
#[component]
fn ConsoleIcon(kind: &'static str) -> Element {
    let path = match kind {
        "grid" => "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z",
        "list" => "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
        "clock" => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM12 7v5l4 2",
        "activity" => "M3 12h4l2-7 4 14 2-7h6",
        "shield" => "M12 22s8-3 8-10V5l-8-3-8 3v7c0 7 8 10 8 10z",
        "message" => "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
        "file" => {
            "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 17H8M10 9H8"
        }
        "user" => {
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"
        }
        "ban" => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM4.93 4.93l14.14 14.14",
        "login" => "M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3",
        "gavel" => "m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10M16 16l6-6M8 8l6-6M9 7l8 8M21 11l-8-8",
        "chevron" => "m15 18-6-6 6-6",
        _ => "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20z",
    };
    rsx! {
        svg {
            class: "console-nav__icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: path }
        }
    }
}

#[component]
fn ConsoleNavLink(
    to: Route,
    label: String,
    icon: &'static str,
    active: bool,
    on_navigate: EventHandler<()>,
) -> Element {
    rsx! {
        Link {
            class: "console-nav__link",
            to,
            aria_current: if active { "page" } else { "false" },
            title: label.clone(),
            onclick: move |_| on_navigate.call(()),
            ConsoleIcon { kind: icon }
            span { class: "console-nav__label", "{label}" }
        }
    }
}

/// 公开端分组侧栏；当前路由只从外壳传入，不复制任何查询或命令状态。
#[component]
pub(crate) fn ConsoleSidebar(
    current_route: Route,
    collapsed: bool,
    on_toggle: EventHandler<()>,
    on_navigate: EventHandler<()>,
) -> Element {
    let home_active = matches!(&current_route, Route::AuctionIndex {} | Route::AuctionPage {});
    let is = |route: &Route| current_route == *route;
    let collapse_label = if collapsed { t!("console-expand") } else { t!("console-collapse") };

    rsx! {
        aside {
            class: if collapsed {
                "console-sidebar is-collapsed"
            } else {
                "console-sidebar"
            },
            aria_label: t!("nav-label"),
            div { class: "console-sidebar__brand",
                span { class: "console-sidebar__brand-name", {t!("app-title")} }
            }
            nav { class: "console-nav",
                div { class: "console-nav__group",
                    div { class: "console-nav__head", {t!("console-nav-overview")} }
                    ConsoleNavLink {
                        to: Route::AuctionIndex {},
                        icon: "grid",
                        label: t!("nav-auctions"),
                        active: home_active,
                        on_navigate,
                    }
                    ConsoleNavLink {
                        to: Route::BidLog {},
                        icon: "list",
                        label: t!("page-bidlog-title"),
                        active: is(&Route::BidLog {}),
                        on_navigate,
                    }
                }
                div { class: "console-nav__group",
                    div { class: "console-nav__head", {t!("console-nav-market")} }
                    ConsoleNavLink {
                        to: Route::AuctionList {},
                        icon: "clock",
                        label: t!("nav-auction-history"),
                        active: is(&Route::AuctionList {}),
                        on_navigate,
                    }
                    ConsoleNavLink {
                        to: Route::History {},
                        icon: "activity",
                        label: t!("nav-price-history"),
                        active: is(&Route::History {}),
                        on_navigate,
                    }
                    ConsoleNavLink {
                        to: Route::CompensationPool {},
                        icon: "shield",
                        label: t!("page-compensation-pool-title"),
                        active: is(&Route::CompensationPool {}),
                        on_navigate,
                    }
                }
                div { class: "console-nav__group",
                    div { class: "console-nav__head", {t!("console-nav-community")} }
                    ConsoleNavLink {
                        to: Route::Forum {},
                        icon: "message",
                        label: t!("nav-forum"),
                        active: is(&Route::Forum {}),
                        on_navigate,
                    }
                    ConsoleNavLink {
                        to: Route::Rules {},
                        icon: "file",
                        label: t!("page-rules-title"),
                        active: is(&Route::Rules {}),
                        on_navigate,
                    }
                    ConsoleNavLink {
                        to: Route::Blacklist {},
                        icon: "ban",
                        label: t!("nav-blacklist"),
                        active: is(&Route::Blacklist {}),
                        on_navigate,
                    }
                }
            }
            div { class: "console-sidebar__foot",
                button {
                    class: "console-collapse",
                    r#type: "button",
                    aria_expanded: (!collapsed).to_string(),
                    title: collapse_label.clone(),
                    onclick: move |_| on_toggle.call(()),
                    ConsoleIcon { kind: "chevron" }
                    span { "{collapse_label}" }
                }
            }
        }
    }
}
