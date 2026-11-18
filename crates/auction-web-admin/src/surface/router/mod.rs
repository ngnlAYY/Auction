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
    renames, returns, rules, settlement, status, support, users,
};

/// 冷启动的查询和片段只保存在当前页面内存。
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct InitialAdminLocation(Rc<RefCell<Option<AdminLocationSuffix>>>);

#[cfg(any(target_arch = "wasm32", test))]
struct AdminLocationSuffix {
    pathname: String,
    search: String,
    hash: String,
}

/// 在 Router 规范化地址前保留未在 Routable 声明的查询与片段。
pub(super) fn use_initial_admin_location() {
    #[cfg(target_arch = "wasm32")]
    use_context_provider(|| {
        let initial = web_sys::window().and_then(|window| {
            let location = window.location();
            let search = location.search().ok()?;
            let pathname = location.pathname().ok()?;
            let hash = location.hash().ok()?;
            capture_initial_admin_location(&pathname, &search, &hash)
        });
        InitialAdminLocation(Rc::new(RefCell::new(initial)))
    });
}

#[cfg(any(target_arch = "wasm32", test))]
fn capture_initial_admin_location(
    pathname: &str,
    search: &str,
    hash: &str,
) -> Option<AdminLocationSuffix> {
    if (search.is_empty() && hash.is_empty())
        || !pathname.starts_with('/')
        || pathname.starts_with("//")
        || pathname.contains('\\')
    {
        return None;
    }
    Some(AdminLocationSuffix {
        pathname: pathname.to_owned(),
        search: search.to_owned(),
        hash: hash.to_owned(),
    })
}

#[cfg(any(target_arch = "wasm32", test))]
fn initial_admin_url(
    initial: &AdminLocationSuffix,
    pathname: &str,
    search: &str,
    hash: &str,
) -> Option<String> {
    // 仅补回 Router 去掉的后缀，已有的新查询、片段或导航优先。
    if (pathname != initial.pathname && initial.pathname.strip_suffix('/') != Some(pathname))
        || (!search.is_empty() && search != initial.search)
        || (!hash.is_empty() && hash != initial.hash)
        || (search == initial.search && hash == initial.hash)
    {
        return None;
    }
    Some(format!("{pathname}{}{}", initial.search, initial.hash))
}

/// 认证请求前恢复一次冷启动 URL，保留 Router 已写入的历史状态。
pub(super) fn restore_initial_admin_location() {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(initial) = try_consume_context::<InitialAdminLocation>() else { return };
        let Some(initial) = initial.0.borrow_mut().take() else { return };
        let Some(window) = web_sys::window() else { return };
        let location = window.location();
        let (Ok(pathname), Ok(search), Ok(hash)) =
            (location.pathname(), location.search(), location.hash())
        else {
            return;
        };
        let Some(url) = initial_admin_url(&initial, &pathname, &search, &hash) else { return };
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
fn SettlementReturns() -> Element {
    returns::page()
}

#[component]
fn SettlementReturnDetail(return_id: String) -> Element {
    returns::detail_page(return_id)
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

#[cfg(test)]
mod tests {
    use super::{capture_initial_admin_location, initial_admin_url};

    #[test]
    fn admin_bookmark_keeps_query_and_fragment_independently() {
        let bookmark =
            capture_initial_admin_location("/dashboard", "", "#dashboard-title").unwrap();
        assert_eq!(
            initial_admin_url(&bookmark, "/dashboard", "", "").as_deref(),
            Some("/dashboard#dashboard-title")
        );
        let bookmark = capture_initial_admin_location("/audit", "?page=2", "").unwrap();
        assert_eq!(
            initial_admin_url(&bookmark, "/audit", "", "").as_deref(),
            Some("/audit?page=2")
        );
        let bookmark = capture_initial_admin_location("/audit/", "?page=2", "#audit").unwrap();
        assert_eq!(
            initial_admin_url(&bookmark, "/audit", "?page=2", "").as_deref(),
            Some("/audit?page=2#audit")
        );
        assert!(capture_initial_admin_location("/audit", "", "").is_none());
        assert!(capture_initial_admin_location("//example.test", "?page=2", "").is_none());
        assert!(initial_admin_url(&bookmark, "/users", "", "").is_none());
        assert!(initial_admin_url(&bookmark, "/audit", "?page=3", "").is_none());
        assert!(initial_admin_url(&bookmark, "/audit", "", "#other").is_none());
        assert!(initial_admin_url(&bookmark, "/audit", "?page=2", "#audit").is_none());
    }
}
