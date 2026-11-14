//! Public Dioxus 路由表、语言目录与启动入口；页面外壳见 `shell` 模块。

use auction_web_shared::i18n::{
    DEFAULT_PUBLIC_LOCALE, LocaleCatalog, LocaleSession, LocaleSource, build_bundle,
};
use auction_web_shared::{
    BaseStylesheet, ConsoleStylesheet, StaleBuildBanner, t, use_locale_effects, use_theme_effects,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use unic_langid::langid;

use super::pages::History;
use super::session::initial_login_return_location;
use super::shell::PublicShell;
use super::{
    About, Account, AuctionIndex, AuctionList, AuctionPage, BidLog, Blacklist, Compensation,
    CompensationPool, Forum, Login, LotDetail, Me, NotFound, Rules, Status,
    initial_public_location,
};

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css", AssetOptions::css().with_static_head(true));
const DESKTOP_CSS: Asset =
    asset!("/assets/desktop.css", AssetOptions::css().with_static_head(true));
const MOBILE_CSS: Asset = asset!("/assets/mobile.css", AssetOptions::css().with_static_head(true));

/// Public 语言目录：默认英文编译期内嵌，其余四种从带指纹的静态资源按需取回。
pub(crate) static PUBLIC_LOCALE_CATALOG: LocaleCatalog = LocaleCatalog {
    default_locale: DEFAULT_PUBLIC_LOCALE,
    default_ftl: include_str!("../../i18n/en.ftl"),
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

/// 公开端路由表。
/// 路径逐条登记旧系统已冻结的公开端 route 与经评审的 Public 扩展，新增前必须确认外部契约。
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
// 路径字面量必须与 auction-http/src/assets/routes.rs 的 Public 清单同步，未知路径不能获得 SPA 入口。
pub(crate) enum Route {
    #[layout(PublicShell)]
        #[route("/")]
        AuctionIndex {},

        #[route("/auction")]
        AuctionPage {},

        #[route("/auctions")]
        AuctionList {},

        #[route("/lots/:lot_code")]
        LotDetail { lot_code: String },

        #[route("/bidlog")]
        BidLog {},

        #[route("/history")]
        History {},

        #[route("/login?:return_to")]
        Login { return_to: Option<String> },

        #[route("/me")]
        Me {},

        #[route("/account")]
        Account {},

        #[route("/rules")]
        Rules {},

        #[route("/about")]
        About {},

        #[route("/blacklist")]
        Blacklist {},

        #[route("/compensation-pool")]
        CompensationPool {},

        #[route("/compensation")]
        Compensation {},

        #[route("/forum")]
        Forum {},

        #[route("/status")]
        Status {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
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

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: PUBLIC_CSS }
        document::Stylesheet { href: DESKTOP_CSS }
        document::Stylesheet { href: MOBILE_CSS }
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
