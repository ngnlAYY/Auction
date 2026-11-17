//! Public 语言目录与启动入口；路由表见子模块 `route`，页面外壳见 `shell` 模块。

// dioxus-router-macro 0.7.10 的 `Routable` 派生把 `String`/`Vec<String>` 段的解析错误包进
// `map_err` 闭包；Rust 1.100 起 `Infallible` 即 `!`，该闭包被判为不可达调用。派生代码在 `const _`
// 块中与 `Route` 并列，枚举上的 lint 属性覆盖不到，因此路由表单独成子模块，只对该子模块放行，
// 本模块其余代码照常受 lint 约束；上游修正派生后删除。
#[allow(unreachable_code, reason = "Routable 派生的段解析错误映射在 Infallible 即 ! 后不可达")]
mod route;

pub(crate) use route::Route;

use auction_web_shared::i18n::{
    DEFAULT_PUBLIC_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle,
};
use auction_web_shared::{
    BaseStylesheet, StaleBuildBanner, t, use_locale_effects, use_theme_effects,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use unic_langid::langid;

use super::initial_public_location;
use super::session::initial_login_return_location;

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css", AssetOptions::css().with_static_head(true));
const DESKTOP_CSS: Asset =
    asset!("/assets/desktop.css", AssetOptions::css().with_static_head(true));
const MOBILE_CSS: Asset = asset!("/assets/mobile.css", AssetOptions::css().with_static_head(true));

/// Public 语言目录：默认英文编译期内嵌，其余四种从带指纹的静态资源按需取回。
pub(crate) static PUBLIC_LOCALE_CATALOG: LocaleCatalog = LocaleCatalog {
    default_locale: DEFAULT_PUBLIC_LOCALE,
    default_ftl: include_str!("../../../i18n/en.ftl"),
    sources: &[
        LocaleSource { id: langid!("zh-CN"), asset: asset!("/i18n/zh-CN.ftl") },
        LocaleSource { id: langid!("zh-TW"), asset: asset!("/i18n/zh-TW.ftl") },
        LocaleSource { id: langid!("ja"), asset: asset!("/i18n/ja.ftl") },
        LocaleSource { id: langid!("ko"), asset: asset!("/i18n/ko.ftl") },
    ],
};

pub fn run() {
    dioxus::launch(App);
}

#[component]
pub(crate) fn App() -> Element {
    // Router 会规范化未声明的 query；在它挂载前分别保留页面查询状态和认证返回来源。
    let initial_location = initial_public_location();
    let provided_initial_location = initial_location.clone();
    use_context_provider(move || provided_initial_location);
    use_context_provider(initial_login_return_location);
    // 默认语言编译期内嵌；其余语言按目录从静态资源取回，已保存的非默认语言在首屏等待加载。
    let i18n_bundle = try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_PUBLIC_LOCALE,
            PUBLIC_LOCALE_CATALOG.default_ftl,
            &DEFAULT_PUBLIC_LOCALE,
            None,
        )
    });
    let Ok(i18n_bundle) = i18n_bundle else {
        return rsx! {
            div { class: "app-root", "Unable to initialize translations." }
        };
    };
    let mut session = use_hook(|| LocaleSession::new(i18n_bundle, DEFAULT_PUBLIC_LOCALE));
    use_hook(|| session.bootstrap(&PUBLIC_LOCALE_CATALOG));
    use_context_provider(|| session);
    use_locale_effects(session);
    // 首次渲染前恢复 hv.theme，应用显示时已是目标主题。
    use_theme_effects();
    // Router 只在语言就绪后挂载：已保存的非默认语言首帧只渲染空壳，此时作废快照会让
    // 冷加载地址里的 query 在 Router 规范化后无从恢复。快照保留到 Router 首次渲染结束。
    let router_rendered = session.is_ready();
    use_effect(use_reactive((&router_rendered,), move |(router_rendered,)| {
        if router_rendered {
            initial_location.expire();
        }
    }));
    if !router_rendered {
        return rsx! {
            div { class: "app-root" }
        };
    }

    // 样式只由静态 head 声明；这里的引用让三份 surface 样式进入构建产物。
    let _ = (PUBLIC_CSS, DESKTOP_CSS, MOBILE_CSS);
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
