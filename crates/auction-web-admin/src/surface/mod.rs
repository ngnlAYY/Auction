//! 管理端浏览器界面（Dioxus CSR）。
//! 路由集合由 `router/route.rs` 定义，与 `crates/auction-http/src/assets/routes.rs` 的 Admin 清单及
//! `docs/contracts/routes.md` 逐条同步。管理端与公开端是不同 surface：本 crate 独立拥有
//! 自己的组件树和路由树，不复用公开端组件，公开端也不得依赖本 crate。
//! 界面文案取自 `i18n/*.ftl`，默认语言由 `auction_web_shared::i18n::DEFAULT_ADMIN_LOCALE`
//! （简体中文）决定。受支持语言与公开端一致，但译文资源彼此隔离。
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
mod returns;
mod router;
mod rules;
mod settlement;
mod setup;
mod sort;
mod status;
mod support;
#[cfg(test)]
mod test_events;
mod users;
mod validation;

use auction_web_shared::{
    BaseStylesheet, StaleBuildBanner,
    i18n::{DEFAULT_ADMIN_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle},
    t, use_locale_effects, use_theme_effects,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use unic_langid::langid;

pub(super) use command::{run_admin_action, run_admin_feedback_action};
pub(super) use presentation::{
    AdminActionFeedback, admin_api_error_message, admin_world_label, format_admin_time,
    lot_status_label,
};
use router::Route;

/// Admin 登录外壳唯一拥有的 readiness 快照；默认 Dashboard 用它协调首屏门禁。
pub(crate) type AdminReadinessResource = Resource<Result<bool, String>>;

/// 三份样式均声明相同层顺序；同层规则的加载先后由 `ci/release/web-bundle.sh` 固定。
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
    let default_ftl = ADMIN_LOCALE_CATALOG.default_ftl;
    #[cfg(test)]
    let default_ftl = try_consume_context::<tests::DefaultFtl>().map_or(default_ftl, |ftl| ftl.0);
    let initialized = try_use_init_i18n_from_bundle(|| {
        build_bundle(&DEFAULT_ADMIN_LOCALE, default_ftl, &DEFAULT_ADMIN_LOCALE, None)
    });
    match initialized {
        Ok(_bundle) => rsx! { LocalizedApp {} },
        Err(_error) => rsx! { div { class: "app-root", "无法初始化界面译文。" } },
    }
}

/// 译文初始化成功后才挂载语言与主题的 hook 宿主。
#[component]
fn LocalizedApp() -> Element {
    let mut session = use_hook(|| LocaleSession::new(i18n_bundle(), DEFAULT_ADMIN_LOCALE));
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

    // 样式只由静态 head 声明；这里的引用让三份 surface 样式进入构建产物。
    let _ = (ADMIN_CSS, ADMIN_DESKTOP_CSS, ADMIN_MOBILE_CSS);
    rsx! {
        BaseStylesheet {}
        div { class: "app-root",
            StaleBuildBanner {
                message: t!("stale-build-notice"),
                refresh_label: t!("stale-build-refresh"),
            }
            Router::<Route> {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use dioxus::core::TemplateNode;
    use dioxus::prelude::*;

    use super::App;

    #[derive(Clone, Copy)]
    pub(super) struct DefaultFtl(pub(super) &'static str);

    #[test]
    fn invalid_default_locale_keeps_the_fallback_through_root_rerenders() {
        let mut dom = VirtualDom::new(App).with_root_context(DefaultFtl("broken = {"));
        dom.rebuild_to_vec();
        for _ in 0..3 {
            let root = dom.get_scope(ScopeId::APP).unwrap().root_node();
            let [TemplateNode::Element { tag: "div", children, .. }] = root.template.roots else {
                panic!("译文初始化失败时应渲染错误容器");
            };
            assert_matches!(children, [TemplateNode::Text { text: "无法初始化界面译文。" }]);
            assert!(root.dynamic_nodes.is_empty());
            dom.mark_dirty(ScopeId::APP);
            dom.render_immediate_to_vec();
        }
    }
}
