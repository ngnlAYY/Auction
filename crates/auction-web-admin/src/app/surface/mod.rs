//! 管理端浏览器界面（Dioxus CSR）。
//! 路由集合按旧版 Auction-BF 的 `manifests/admin.routes.json` 冻结。管理端与公开端是
//! 不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，公开端也不
//! 得依赖本 crate。
//! 界面文案取自 `i18n/*.ftl`，默认简体中文——沿用旧 `manifests/admin.locales.json` 的
//! `default_locale`。受支持语言与公开端一致，但译文资源彼此隔离。
//! 本 crate 不得导入存储、配置或凭据访问路径。

mod alerts;
#[path = "../auction/auctions.rs"]
mod auctions;
#[path = "../operations/audit.rs"]
mod audit;
mod auth;
mod command;
#[path = "../operations/config.rs"]
mod config;
#[path = "../identity/credentials.rs"]
mod credentials;
#[path = "../operations/dashboard.rs"]
mod dashboard;
#[path = "../fulfillment/deliveries/mod.rs"]
mod deliveries;
#[path = "../auction/intake.rs"]
mod intake;
#[path = "../auction/lots.rs"]
mod lots;
#[path = "../fulfillment/mail.rs"]
mod mail;
mod pagination;
mod pending;
#[path = "../poll.rs"]
mod poll;
#[path = "../query.rs"]
mod query;
#[path = "../operations/realtime/mod.rs"]
mod realtime;
#[path = "../identity/renames.rs"]
mod renames;
mod router;
#[path = "../content/rules/mod.rs"]
mod rules;
#[path = "../auction/settlement.rs"]
mod settlement;
#[path = "../operations/setup.rs"]
mod setup;
mod shared;
#[path = "../sort.rs"]
mod sort;
#[path = "../operations/status.rs"]
mod status;
mod support;
#[path = "../identity/users.rs"]
mod users;
#[path = "../validation.rs"]
mod validation;

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::langid;

use auction_web_shared::{
    BaseStylesheet, StaleBuildBanner,
    i18n::{DEFAULT_ADMIN_LOCALE, initial_language},
    legacy_t as t, use_locale_effects, use_theme_effects,
};

pub(super) use command::{run_admin_action, run_admin_feedback_action};
use router::Route;
pub(super) use shared::{
    AdminActionFeedback, admin_api_error_message, admin_world_label, format_admin_time,
};

/// Admin 登录外壳唯一拥有的 readiness 快照；默认 Dashboard 用它协调首屏门禁。
pub(crate) type AdminReadinessResource = Resource<Result<bool, String>>;

/// 三份样式均声明相同层顺序，不依赖静态 head 资源的输出先后。
const ADMIN_CSS: Asset = asset!("/assets/admin.css", AssetOptions::css().with_static_head(true));
const ADMIN_DESKTOP_CSS: Asset =
    asset!("/assets/desktop.css", AssetOptions::css().with_static_head(true));
const ADMIN_MOBILE_CSS: Asset =
    asset!("/assets/mobile.css", AssetOptions::css().with_static_head(true));

pub fn run() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    router::use_initial_admin_location();
    // 语言资源在编译期内嵌；管理端默认简体中文，并沿用已保存的语言偏好。
    let i18n = use_init_i18n(|| {
        I18nConfig::new(initial_language(DEFAULT_ADMIN_LOCALE))
            .with_locale((langid!("en"), include_str!("../../../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../../../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../../../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../../../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../../../i18n/ko.ftl")))
            .with_fallback(DEFAULT_ADMIN_LOCALE)
    });
    use_locale_effects(i18n);
    // 首次渲染前恢复 hv.theme；登录、首次设置与正式页面共享同一主题。
    use_theme_effects();

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: ADMIN_CSS }
        document::Stylesheet { href: ADMIN_DESKTOP_CSS }
        document::Stylesheet { href: ADMIN_MOBILE_CSS }
        div { class: "app-root",
            StaleBuildBanner {
                message: t!("stale-build-notice"),
                refresh_label: t!("stale-build-refresh"),
            }
            Router::<Route> {}
        }
    }
}
