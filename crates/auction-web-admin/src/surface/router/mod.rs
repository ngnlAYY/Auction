//! 管理端路由、页面入口组件和旧版别名跳转；路由表见子模块 `route`。
//! 路由路径与页面组件在同一 owner 内维护，避免 surface 装配文件再次聚合所有页面入口。

// dioxus-router-macro 0.7.10 的 `Routable` 派生把 `String`/`Vec<String>` 段的解析错误包进
// `map_err` 闭包；Rust 1.100 起 `Infallible` 即 `!`，该闭包被判为不可达调用。派生代码在 `const _`
// 块中与 `Route` 并列，枚举上的 lint 属性覆盖不到，因此路由表单独成子模块，只对该子模块放行，
// 本模块的页面入口与别名跳转照常受 lint 约束；上游修正派生后删除。
#[allow(unreachable_code, reason = "Routable 派生的段解析错误映射在 Infallible 即 ! 后不可达")]
mod route;

pub(super) use route::Route;

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

use auction_web_shared::t;
use dioxus::prelude::*;

use super::{
    alerts, auctions, audit, config, credentials, dashboard, deliveries, lots, mail, realtime,
    renames, rules, settlement, status, support, users,
};

/// 仅在冷启动含 query 时保存一次原始相对地址，不持久化或记录查询值。
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct InitialAdminLocation(Option<Rc<RefCell<Option<String>>>>);

/// 与 Public 相同，在 Router 规范化地址前保留未在 Routable 声明的查询。
pub(super) fn use_initial_admin_location() {
    #[cfg(target_arch = "wasm32")]
    use_context_provider(|| {
        let initial = web_sys::window().and_then(|window| {
            let location = window.location();
            let search = location.search().ok()?;
            if search.is_empty() {
                return None;
            }
            let mut url = location.pathname().ok()?;
            let hash = location.hash().ok()?;
            url.reserve(search.len() + hash.len());
            url.push_str(&search);
            url.push_str(&hash);
            Some(url)
        });
        InitialAdminLocation(initial.map(|url| Rc::new(RefCell::new(Some(url)))))
    });
}

/// 认证请求前恢复当前路径的首次 query；只消费一次，不覆盖后来导航或已有的新查询。
pub(super) fn restore_initial_admin_location() {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(initial) =
            try_consume_context::<InitialAdminLocation>().and_then(|location| location.0)
        else {
            return;
        };
        let Some(url) = initial.borrow_mut().take() else { return };
        let Some((pathname, _)) = url.split_once('?') else { return };
        let Some(window) = web_sys::window() else { return };
        let location = window.location();
        if location.pathname().ok().as_deref() != Some(pathname)
            || location.search().ok().as_deref() != Some("")
        {
            return;
        }
        if let Ok(history) = window.history() {
            let state = history.state().unwrap_or(wasm_bindgen::JsValue::NULL);
            let _ = history.replace_state_with_url(&state, "", Some(&url));
        }
    }
}

#[component]
fn Setup() -> Element {
    super::setup::page()
}

#[component]
fn Dashboard() -> Element {
    dashboard::page()
}

#[component]
fn DashboardAlias() -> Element {
    redirect_admin_alias("/")
}

#[component]
fn Login() -> Element {
    // 未认证时由 AdminShell 显示登录表单；已认证后访问旧登录路径回到总览。
    dashboard::page()
}

#[component]
fn Alerts() -> Element {
    alerts::page()
}

#[component]
fn Auctions() -> Element {
    auctions::page()
}

#[component]
fn AuctionLots(auction_id: String) -> Element {
    lots::page(auction_id)
}

#[component]
fn AuctionOverview() -> Element {
    redirect_admin_alias("/auctions")
}

#[component]
fn Mails() -> Element {
    mail::page()
}

#[component]
fn MailOverview() -> Element {
    redirect_admin_alias("/mails")
}

#[component]
fn Cookies() -> Element {
    // /cookies 是凭据管理的正式入口；/credentials 只作为兼容旧书签的别名重定向到这里。
    credentials::page()
}

#[component]
fn Credentials() -> Element {
    redirect_admin_alias("/cookies")
}

#[component]
fn Users() -> Element {
    users::page()
}

#[component]
fn Renames() -> Element {
    renames::page()
}

#[component]
fn Deliveries() -> Element {
    deliveries::page()
}

#[component]
fn DeliveryDetail(delivery_id: String) -> Element {
    deliveries::detail_page(delivery_id)
}

#[component]
fn DeliveryOverview() -> Element {
    redirect_admin_alias("/deliveries")
}

#[component]
fn PlayerSupport() -> Element {
    support::page()
}

#[component]
fn Outbox() -> Element {
    realtime::outbox_page()
}

#[component]
fn Settlement() -> Element {
    settlement::page()
}

#[component]
fn Config() -> Element {
    config::page()
}

#[component]
fn RulesContent() -> Element {
    rules::page(crate::api::AdminDocumentKind::Rules)
}

#[component]
fn AboutContent() -> Element {
    rules::page(crate::api::AdminDocumentKind::About)
}

#[component]
fn Rules() -> Element {
    redirect_admin_alias("/rules-content")
}

#[component]
fn Audit() -> Element {
    audit::page()
}

#[component]
fn Status() -> Element {
    status::page()
}

/// 把旧版管理端别名无历史记录地收敛到 canonical URL，并保留查询字符串和片段。
/// Admin 旧链接仍保留在路由表中以兼容书签；`replace` 确保返回键不会在别名与正式地址间
/// 形成额外的导航环。片段由旧版路由行为保留，便于外部链接定位页面内锚点。
fn redirect_admin_alias(path: &'static str) -> Element {
    let target = format!("{path}{}", current_browser_suffix());
    use_effect(move || {
        let _ = navigator().replace(target.clone());
    });
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn current_browser_suffix() -> String {
    let Some(window) = web_sys::window() else {
        return String::new();
    };
    let location = window.location();
    let search = location.search().unwrap_or_default();
    let hash = location.hash().unwrap_or_default();
    format!("{search}{hash}")
}

#[cfg(not(target_arch = "wasm32"))]
fn current_browser_suffix() -> String {
    String::new()
}

/// 404 位于认证外壳的 `main#admin-content` 之内：只输出页面区块，不再嵌套第二个 `main`
/// 地标，也不套用整屏高度的外壳类。
#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-not-found-title",
            h1 { id: "admin-not-found-title", {t!("not-found-title")} }
            p { class: "admin-panel__note", {t!("not-found-note")} }
        }
    }
}
