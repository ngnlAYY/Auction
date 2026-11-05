//! 管理端路由表、页面入口组件和旧版别名跳转。
//! 路由路径与页面组件在同一 owner 内维护，避免 surface 装配文件再次聚合所有页面入口。

use super::auth::AdminShell;
use super::{
    auctions, audit, config, credentials, dashboard, deliveries, lots, mail, realtime, renames,
    rules, settlement, status, support, users,
};
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use std::{cell::RefCell, rc::Rc};

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

/// 管理端路由表。
/// 路径逐条对应旧系统已冻结的管理端 route。管理命令的实现必须走版本化 REST 契约，
/// 不在浏览器端重建状态机。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Admin 清单同步，未知路径不能获得 SPA 入口。
pub(super) enum Route {
    #[route("/setup")]
    Setup {},

    #[layout(AdminShell)]
        #[route("/")]
        Dashboard {},

        #[route("/dashboard")]
        DashboardAlias {},

        #[route("/login")]
        Login {},

        #[route("/auctions")]
        Auctions {},

        #[route("/auctions/:auction_id/lots")]
        AuctionLots { auction_id: String },

        #[route("/auction")]
        AuctionOverview {},

        #[route("/mails")]
        Mails {},

        #[route("/mail")]
        MailOverview {},

        #[route("/cookies")]
        Cookies {},

        #[route("/credentials")]
        Credentials {},

        #[route("/users")]
        Users {},

        #[route("/renames")]
        Renames {},

        #[route("/deliveries")]
        Deliveries {},

        /// 单条投递的只读证据详情；玩家报出的投递 Reference 可直接定位。
        #[route("/deliveries/:delivery_id")]
        DeliveryDetail { delivery_id: String },

        #[route("/delivery")]
        DeliveryOverview {},

        /// W4.6 玩家支持：按用户或参考编号只读汇总，一次检索定位相关记录。
        #[route("/player-support")]
        PlayerSupport {},

        #[route("/outbox")]
        Outbox {},

        #[route("/settlement")]
        Settlement {},

        #[route("/config")]
        Config {},

        #[route("/rules-content")]
        RulesContent {},

        #[route("/rules")]
        Rules {},

        #[route("/audit")]
        Audit {},

        #[route("/status")]
        Status {},

        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
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
    // Auction-BF 的 /cookies 是凭据管理入口，/credentials 是迁移期间保留的明确别名。
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
    rules::page()
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

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        main { class: "admin-shell",
            section { class: "admin-panel",
                h1 { {t!("not-found-title")} }
                p { class: "admin-panel__note", {t!("not-found-note")} }
            }
        }
    }
}
