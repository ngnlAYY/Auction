//! 管理端浏览器界面（Dioxus CSR）。
//! 路由集合按旧版 Auction-BF 的 `manifests/admin.routes.json` 冻结。管理端与公开端是
//! 不同 surface：本 crate 独立拥有自己的组件树和路由树，不复用公开端组件，公开端也不
//! 得依赖本 crate。
//! 界面文案取自 `i18n/*.ftl`，默认简体中文——沿用旧 `manifests/admin.locales.json` 的
//! `default_locale`。受支持语言与公开端一致，但译文资源彼此隔离。
//! 本 crate 不得导入存储、配置或凭据访问路径。

mod alerts;
mod auctions;
mod audit;
mod auth;
mod command;
mod config;
mod credentials;
mod dashboard;
mod deliveries;
mod intake;
mod lots;
mod mail;
mod pagination;
mod pending;
mod poll;
mod presentation;
mod query;
mod realtime;
mod renames;
mod router;
mod rules;
mod settlement;
mod setup;
mod sort;
mod status;
mod support;
mod users;
mod validation;

use auction_web_shared::{
    BaseStylesheet, ConsoleStylesheet, StaleBuildBanner,
    i18n::{DEFAULT_ADMIN_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle},
    t, use_locale_effects, use_theme_effects,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::langid;

pub(super) use command::{run_admin_action, run_admin_feedback_action};
pub(super) use presentation::{
    AdminActionFeedback, admin_api_error_message, admin_world_label, format_admin_time,
};
use router::Route;

/// Admin 登录外壳唯一拥有的 readiness 快照；默认 Dashboard 用它协调首屏门禁。
pub(crate) type AdminReadinessResource = Resource<Result<bool, String>>;

/// 三份样式均声明相同层顺序，不依赖静态 head 资源的输出先后。
const ADMIN_CSS: Asset = asset!("/assets/admin.css", AssetOptions::css().with_static_head(true));
const ADMIN_DESKTOP_CSS: Asset =
    asset!("/assets/desktop.css", AssetOptions::css().with_static_head(true));
const ADMIN_MOBILE_CSS: Asset =
    asset!("/assets/mobile.css", AssetOptions::css().with_static_head(true));

/// Admin 语言目录：默认简体中文编译期内嵌，其余四种从带指纹的静态资源按需取回。
pub(crate) static ADMIN_LOCALE_CATALOG: LocaleCatalog = LocaleCatalog {
    default_locale: DEFAULT_ADMIN_LOCALE,
    default_ftl: include_str!("../../i18n/zh-CN.ftl"),
    sources: &[
        LocaleSource { id: langid!("en"), asset: asset!("/i18n/en.ftl") },
        LocaleSource { id: langid!("zh-TW"), asset: asset!("/i18n/zh-TW.ftl") },
        LocaleSource { id: langid!("ja"), asset: asset!("/i18n/ja.ftl") },
        LocaleSource { id: langid!("ko"), asset: asset!("/i18n/ko.ftl") },
    ],
};

pub fn run() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    router::use_initial_admin_location();
    // 默认语言编译期内嵌；其余语言按目录从静态资源取回，已保存的非默认语言在首屏等待加载。
    let i18n_bundle = try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_ADMIN_LOCALE,
            ADMIN_LOCALE_CATALOG.default_ftl,
            &DEFAULT_ADMIN_LOCALE,
            None,
        )
    });
    let Ok(i18n_bundle) = i18n_bundle else {
        return rsx! {
            div { class: "app-root", "无法初始化界面译文。" }
        };
    };
    let mut session = use_hook(|| LocaleSession::new(i18n_bundle, DEFAULT_ADMIN_LOCALE));
    use_hook(|| session.bootstrap(&ADMIN_LOCALE_CATALOG));
    use_context_provider(|| session);
    use_locale_effects(session);
    // 首次渲染前恢复 hv.theme；登录、首次设置与正式页面共享同一主题。
    use_theme_effects();
    if !session.is_ready() {
        return rsx! {
            div { class: "app-root" }
        };
    }

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: ADMIN_CSS }
        document::Stylesheet { href: ADMIN_DESKTOP_CSS }
        document::Stylesheet { href: ADMIN_MOBILE_CSS }
        ConsoleStylesheet {}
        div { class: "app-root",
            StaleBuildBanner {
                message: t!("stale-build-notice"),
                refresh_label: t!("stale-build-refresh"),
            }
            Router::<Route> {}
        }
    }
}
