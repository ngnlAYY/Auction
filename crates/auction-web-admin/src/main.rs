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

use dioxus::prelude::*;
use dioxus_i18n::{prelude::*, t};
use unic_langid::langid;

use auction_web_shared::{
    BaseStylesheet, LanguagePicker,
    i18n::{DEFAULT_ADMIN_LOCALE, initial_language},
    use_locale_effects,
};

/// 管理端 surface 样式。设计变量由 `auction-web-shared` 提供。
const ADMIN_CSS: Asset = asset!("/assets/admin.css");

fn main() {
    dioxus::launch(App);
}

/// 管理端路由表。
///
/// 路径逐条对应旧系统已冻结的管理端 route。管理命令的实现必须走版本化 REST 契约，
/// 不在浏览器端重建状态机。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
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
            .with_locale((langid!("en"), include_str!("../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../i18n/ko.ftl")))
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

/// 管理端外壳。与公开端外壳完全独立，导航项覆盖管理专属能力。
#[component]
fn AdminShell() -> Element {
    rsx! {
        main { class: "auction-shell admin-shell",
            nav { class: "admin-nav", aria_label: t!("nav-label"),
                Link { to: Route::Dashboard {}, {t!("nav-dashboard")} }
                Link { to: Route::Auctions {}, {t!("nav-auctions")} }
                Link { to: Route::Users {}, {t!("nav-users")} }
                Link { to: Route::Mails {}, {t!("nav-mails")} }
                Link { to: Route::Deliveries {}, {t!("nav-deliveries")} }
                Link { to: Route::Settlement {}, {t!("nav-settlement")} }
                Link { to: Route::Audit {}, {t!("nav-audit")} }
                Link { to: Route::Config {}, {t!("nav-config")} }
                Link { to: Route::Status {}, {t!("nav-status")} }
                LanguagePicker { label: t!("language-label") }
            }
            Outlet::<Route> {}
        }
    }
}

/// 管理端占位面板。
///
/// 每个管理能力都要在对应 feature 的契约、命令用例和审计边界完成后替换为真实实现。
#[component]
fn AdminPanel(title: String, detail: String) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-panel-title",
            h1 { id: "admin-panel-title", "{title}" }
            p { class: "admin-panel__note", "{detail}" }
        }
    }
}

#[component]
fn Dashboard() -> Element {
    rsx! {
        AdminPanel { title: t!("page-dashboard-title"), detail: t!("page-dashboard-note") }
    }
}

#[component]
fn DashboardAlias() -> Element {
    rsx! {
        AdminPanel { title: t!("page-dashboard-title"), detail: t!("page-dashboard-note") }
    }
}

#[component]
fn Login() -> Element {
    rsx! {
        AdminPanel { title: t!("page-login-title"), detail: t!("page-login-note") }
    }
}

#[component]
fn Auctions() -> Element {
    rsx! {
        AdminPanel { title: t!("page-auctions-title"), detail: t!("page-auctions-note") }
    }
}

#[component]
fn AuctionLots(auction_id: String) -> Element {
    rsx! {
        AdminPanel {
            title: t!("page-auction-lots-title", auction: auction_id.clone()),
            detail: t!("page-auction-lots-note"),
        }
    }
}

#[component]
fn AuctionOverview() -> Element {
    rsx! {
        AdminPanel { title: t!("page-auction-overview-title"), detail: t!("page-auction-overview-note") }
    }
}

#[component]
fn Mails() -> Element {
    rsx! {
        AdminPanel { title: t!("page-mails-title"), detail: t!("page-mails-note") }
    }
}

#[component]
fn MailOverview() -> Element {
    rsx! {
        AdminPanel { title: t!("page-mail-overview-title"), detail: t!("page-mail-overview-note") }
    }
}

#[component]
fn Cookies() -> Element {
    rsx! {
        AdminPanel { title: t!("page-cookies-title"), detail: t!("page-cookies-note") }
    }
}

#[component]
fn Credentials() -> Element {
    rsx! {
        AdminPanel { title: t!("page-credentials-title"), detail: t!("page-credentials-note") }
    }
}

#[component]
fn Users() -> Element {
    rsx! {
        AdminPanel { title: t!("page-users-title"), detail: t!("page-users-note") }
    }
}

#[component]
fn Renames() -> Element {
    rsx! {
        AdminPanel { title: t!("page-renames-title"), detail: t!("page-renames-note") }
    }
}

#[component]
fn Deliveries() -> Element {
    rsx! {
        AdminPanel { title: t!("page-deliveries-title"), detail: t!("page-deliveries-note") }
    }
}

#[component]
fn DeliveryOverview() -> Element {
    rsx! {
        AdminPanel { title: t!("page-delivery-overview-title"), detail: t!("page-delivery-overview-note") }
    }
}

#[component]
fn Outbox() -> Element {
    rsx! {
        AdminPanel { title: t!("page-outbox-title"), detail: t!("page-outbox-note") }
    }
}

#[component]
fn Settlement() -> Element {
    rsx! {
        AdminPanel { title: t!("page-settlement-title"), detail: t!("page-settlement-note") }
    }
}

#[component]
fn Config() -> Element {
    rsx! {
        AdminPanel { title: t!("page-config-title"), detail: t!("page-config-note") }
    }
}

#[component]
fn RulesContent() -> Element {
    rsx! {
        AdminPanel { title: t!("page-rules-content-title"), detail: t!("page-rules-content-note") }
    }
}

#[component]
fn Rules() -> Element {
    rsx! {
        AdminPanel { title: t!("page-rules-title"), detail: t!("page-rules-note") }
    }
}

#[component]
fn Audit() -> Element {
    rsx! {
        AdminPanel { title: t!("page-audit-title"), detail: t!("page-audit-note") }
    }
}

#[component]
fn Status() -> Element {
    rsx! {
        AdminPanel { title: t!("page-status-title"), detail: t!("page-status-note") }
    }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        main { class: "auction-shell admin-shell",
            section { class: "admin-panel",
                h1 { {t!("not-found-title")} }
                p { class: "admin-panel__note", {t!("not-found-note")} }
            }
        }
    }
}
