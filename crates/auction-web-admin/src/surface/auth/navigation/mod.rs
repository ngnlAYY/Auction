//! 桌面侧栏与手机折叠菜单的纯展示；路由、会话与菜单状态仍由认证外壳唯一持有。
//! 两端共用分组导航骨架（概览 / 拍卖运营 / 系统），桌面外壳负责品牌与收起。

pub(super) mod desktop;
pub(super) mod mobile;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use super::super::Route;
use crate::api::AdminPendingCounts;

/// 侧栏图标；统一 18px、1.5 stroke，折叠态仍能识别导航。
#[component]
fn NavigationIcon(kind: &'static str) -> Element {
    let path = match kind {
        "grid" => "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z",
        "bell" => "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0",
        "gavel" => "m14 13-7.5 7.5a2.12 2.12 0 0 1-3-3L11 10M16 16l6-6M8 8l6-6M9 7l8 8M21 11l-8-8",
        "send" => "m22 2-7 20-4-9-9-4ZM22 2 11 13",
        "check" => "M22 11.08V12a10 10 0 1 1-5.93-9.14M22 4 12 14.01l-3-3",
        "inbox" => {
            "M22 12h-6l-2 3h-4l-2-3H2M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"
        }
        "users" => {
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"
        }
        "cookie" => {
            "M12 2a10 10 0 1 0 10 10 4 4 0 0 1-5-5 4 4 0 0 1-5-5M8.5 8.5h.01M16 15.5h.01M12 12h.01M11 17h.01M7 14h.01"
        }
        "edit" => "M12 20h9M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z",
        "message" => "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
        "settings" => {
            "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6h.09A1.65 1.65 0 0 0 10 3.09V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v.09a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
        }
        "file" => {
            "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 17H8M10 9H8"
        }
        "shield" => "M12 22s8-3 8-10V5l-8-3-8 3v7c0 7 8 10 8 10z",
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
fn NavigationLink(
    to: Route,
    label: String,
    icon: &'static str,
    current_route: Route,
    count: i64,
) -> Element {
    let active = current_route == to;
    rsx! {
        Link {
            class: "console-nav__link",
            to,
            aria_current: if active { "page" } else { "false" },
            title: label.clone(),
            NavigationIcon { kind: icon }
            span { class: "console-nav__label", "{label}" }
            if count > 0 {
                span {
                    class: "console-nav__count",
                    title: t!("nav-pending-count", count: count),
                    "{count}"
                }
            }
        }
    }
}

#[component]
pub(super) fn NavigationLinks(current_route: Route) -> Element {
    // 外壳持有唯一待办计数资源；登录过程中尚未取得结果时不显示徽标。
    let counts = try_consume_context::<Resource<Result<AdminPendingCounts, String>>>()
        .and_then(|resource| resource.read().clone())
        .and_then(Result::ok);
    let pending = move |route: &Route| -> i64 {
        let Some(counts) = counts else { return 0 };
        match route {
            Route::Deliveries {} => counts.delivery_manual_reconcile,
            Route::Settlement {} => counts.recovery_confirmation,
            Route::Alerts {} => counts.active_alerts,
            _ => 0,
        }
    };

    rsx! {
        div { class: "console-nav",
            div { class: "console-nav__group",
                div { class: "console-nav__head", {t!("console-nav-overview")} }
                NavigationLink {
                    to: Route::Dashboard {},
                    icon: "grid",
                    label: t!("nav-dashboard"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Alerts {},
                    icon: "bell",
                    label: t!("nav-alerts"),
                    current_route: current_route.clone(),
                    count: pending(&Route::Alerts {}),
                }
            }
            div { class: "console-nav__group",
                div { class: "console-nav__head", {t!("console-nav-auction-ops")} }
                NavigationLink {
                    to: Route::Auctions {},
                    icon: "gavel",
                    label: t!("nav-auctions"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Deliveries {},
                    icon: "send",
                    label: t!("nav-deliveries"),
                    current_route: current_route.clone(),
                    count: pending(&Route::Deliveries {}),
                }
                NavigationLink {
                    to: Route::Settlement {},
                    icon: "check",
                    label: t!("nav-settlement"),
                    current_route: current_route.clone(),
                    count: pending(&Route::Settlement {}),
                }
                NavigationLink {
                    to: Route::Mails {},
                    icon: "inbox",
                    label: t!("nav-mails"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Users {},
                    icon: "users",
                    label: t!("nav-users"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Cookies {},
                    icon: "cookie",
                    label: t!("nav-cookies"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Renames {},
                    icon: "edit",
                    label: t!("nav-renames"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::PlayerSupport {},
                    icon: "message",
                    label: t!("nav-support"),
                    current_route: current_route.clone(),
                    count: 0,
                }
            }
            div { class: "console-nav__group",
                div { class: "console-nav__head", {t!("console-nav-system")} }
                NavigationLink {
                    to: Route::Outbox {},
                    icon: "send",
                    label: t!("page-outbox-title"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Config {},
                    icon: "settings",
                    label: t!("nav-config"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::RulesContent {},
                    icon: "file",
                    label: t!("page-rules-content-title"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::AboutContent {},
                    icon: "file",
                    label: t!("page-about-title"),
                    current_route: current_route.clone(),
                    count: 0,
                }
                NavigationLink {
                    to: Route::Audit {},
                    icon: "shield",
                    label: t!("nav-audit"),
                    current_route: current_route.clone(),
                    count: 0,
                }
            }
        }
    }
}
