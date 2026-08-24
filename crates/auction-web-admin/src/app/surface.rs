//! 管理端浏览器界面（Dioxus CSR）。
//!
//! 路由集合按旧版 Auction-BF 的 `manifests/admin.routes.json` 冻结。管理端与公开端是
//! 不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，公开端也不
//! 得依赖本 crate。
//!
//! 界面文案取自 `i18n/*.ftl`，默认简体中文——沿用旧 `manifests/admin.locales.json` 的
//! `default_locale`。受支持语言与公开端一致，但译文资源彼此隔离。
//!
//! 本 crate 不得导入存储、配置或凭据访问路径。

#[path = "auction/auctions.rs"]
mod auctions;
#[path = "operations/audit.rs"]
mod audit;
#[path = "surface/auth.rs"]
mod auth;
#[path = "operations/config.rs"]
mod config;
#[path = "identity/credentials.rs"]
mod credentials;
#[path = "operations/dashboard.rs"]
mod dashboard;
#[path = "fulfillment/deliveries.rs"]
mod deliveries;
#[path = "auction/intake.rs"]
mod intake;
#[path = "auction/lots.rs"]
mod lots;
#[path = "fulfillment/mail.rs"]
mod mail;
#[path = "shared/pagination.rs"]
mod pagination;
#[path = "operations/realtime/mod.rs"]
mod realtime;
#[path = "identity/renames.rs"]
mod renames;
#[path = "content/rules.rs"]
mod rules;
#[path = "auction/settlement.rs"]
mod settlement;
#[path = "surface/shared.rs"]
mod shared;
#[path = "operations/status.rs"]
mod status;
#[path = "identity/users.rs"]
mod users;

use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use unic_langid::langid;

use auction_web_shared::{
    BaseStylesheet,
    i18n::{DEFAULT_ADMIN_LOCALE, initial_language},
    use_locale_effects,
};

use auth::AdminShell;
pub(super) use shared::{AdminLoadError, AdminPanel, format_admin_time};

/// 管理端 surface 样式。设计变量由 `auction-web-shared` 提供。
const ADMIN_CSS: Asset =
    asset!("/assets/admin.css", AssetOptions::css().with_preload(true).with_static_head(true));

pub fn run() {
    dioxus::launch(App);
}

/// 管理端路由表。
///
/// 路径逐条对应旧系统已冻结的管理端 route。管理命令的实现必须走版本化 REST 契约，
/// 不在浏览器端重建状态机。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Admin 清单同步，未知路径不能获得 SPA 入口。
enum Route {
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

        #[route("/delivery")]
        DeliveryOverview {},

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
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
fn App() -> Element {
    // 语言资源在编译期内嵌；管理端默认简体中文，并沿用已保存的语言偏好。
    let i18n = use_init_i18n(|| {
        I18nConfig::new(initial_language(DEFAULT_ADMIN_LOCALE))
            .with_locale((langid!("en"), include_str!("../../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../../i18n/ko.ftl")))
            .with_fallback(DEFAULT_ADMIN_LOCALE)
    });
    use_locale_effects(i18n);

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: ADMIN_CSS }
        document::Title { {t!("app-title")} }
        Router::<Route> {}
    }
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
fn DeliveryOverview() -> Element {
    redirect_admin_alias("/deliveries")
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
///
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
