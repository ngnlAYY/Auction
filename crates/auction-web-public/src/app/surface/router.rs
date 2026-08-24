//! Public Dioxus 路由、语言初始化和页面外壳。

use super::*;

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset =
    asset!("/assets/public.css", AssetOptions::css().with_preload(true).with_static_head(true));

pub fn run() {
    dioxus::launch(App);
}

/// 公开端路由表。
///
/// 路径逐条对应旧系统已冻结的公开端 route，新增路由前必须先确认外部契约。
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

        #[route("/login")]
        Login {},

        #[route("/me")]
        Me {},

        #[route("/account")]
        Account {},

        #[route("/rules")]
        Rules {},

        #[route("/blacklist")]
        Blacklist {},

        #[route("/compensation-pool")]
        CompensationPool {},

        #[route("/compensation")]
        Compensation {},

        #[route("/status")]
        Status {},
    #[end_layout]

    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[component]
pub(crate) fn App() -> Element {
    // 语言资源在编译期内嵌，避免首屏额外请求；默认英文，并沿用已保存的语言偏好。
    let i18n = use_init_i18n(|| {
        I18nConfig::new(initial_language(DEFAULT_PUBLIC_LOCALE))
            .with_locale((langid!("en"), include_str!("../../../i18n/en.ftl")))
            .with_locale((langid!("zh-CN"), include_str!("../../../i18n/zh-CN.ftl")))
            .with_locale((langid!("zh-TW"), include_str!("../../../i18n/zh-TW.ftl")))
            .with_locale((langid!("ja"), include_str!("../../../i18n/ja.ftl")))
            .with_locale((langid!("ko"), include_str!("../../../i18n/ko.ftl")))
            .with_fallback(DEFAULT_PUBLIC_LOCALE)
    });
    use_locale_effects(i18n);

    rsx! {
        BaseStylesheet {}
        document::Stylesheet { href: PUBLIC_CSS }
        document::Title { {t!("app-title")} }
        Router::<Route> {}
    }
}

/// 公开端外壳：沿用旧版 Auction-BF 的单行页头、导航和居中内容区。
#[component]
pub(crate) fn PublicShell() -> Element {
    use_dialog_focus_trap();
    let selected_world =
        use_signal(|| read_public_query().world.unwrap_or_else(|| "main".to_owned()));
    let mut refresh_generation = use_signal(|| 0_u64);
    let realtime_state = use_signal(|| LiveConnectionState::Checking);
    let realtime_cursor = use_signal(|| 0_i64);
    use_context_provider(|| selected_world);
    use_context_provider(|| refresh_generation);
    use_context_provider(|| realtime_state);

    let _realtime = use_future({
        let refresh_generation = refresh_generation;
        let mut realtime_state = realtime_state;
        move || async move {
            loop {
                realtime_state.set(LiveConnectionState::Checking);
                match fetch_public_session().await {
                    Ok(Some(session)) if session.authenticated => {
                        realtime_state.set(LiveConnectionState::Connecting);
                        let mut callback_state = realtime_state;
                        let mut callback_refresh = refresh_generation;
                        let after = realtime_cursor();
                        let mut callback_cursor = realtime_cursor;
                        match subscribe_realtime(after, move |page| {
                            callback_state.set(LiveConnectionState::Connected);
                            let refetch_required = page.refetch_required;
                            callback_cursor.set(if refetch_required { 0 } else { page.next_after });
                            if refetch_required
                                || page.events.iter().any(should_refresh_auction_board)
                            {
                                callback_refresh.set(callback_refresh().saturating_add(1));
                            }
                        })
                        .await
                        {
                            Ok(()) => realtime_state.set(LiveConnectionState::Stopped),
                            Err(_) => realtime_state.set(LiveConnectionState::Error),
                        }
                    }
                    Ok(_) => realtime_state.set(LiveConnectionState::Anonymous),
                    Err(_) => realtime_state.set(LiveConnectionState::Error),
                }
                TimeoutFuture::new(5_000).await;
            }
        }
    });
    let connection_state = realtime_state();
    let connection_class = live_connection_class(connection_state);
    let connection_message = realtime::live_connection_message(connection_state);
    let can_manual_refresh = matches!(
        connection_state,
        LiveConnectionState::Anonymous | LiveConnectionState::Stopped | LiveConnectionState::Error
    );

    rsx! {
        div { class: "public-shell",
            a { class: "public-shell__skip", href: "#public-content", {t!("skip")} }
            header { class: "public-shell__header",
                Link { class: "public-shell__logo", to: Route::AuctionIndex {},
                    {t!("app-title")}
                }
                nav { class: "public-shell__nav", aria_label: t!("nav-label"),
                    Link { to: Route::AuctionIndex {}, {t!("nav-auctions")} }
                    Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
                    Link { to: Route::AuctionList {}, {t!("page-auctions-title")} }
                    Link { to: Route::History {}, {t!("page-history-title")} }
                    Link { to: Route::Rules {}, {t!("page-rules-title")} }
                    Link { to: Route::Blacklist {}, {t!("page-blacklist-title")} }
                    Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
                }
                div { class: "public-shell__controls",
                    WorldPicker { selected_world }
                    span {
                        class: "public-connection-badge {connection_class}",
                        role: "status",
                        aria_label: t!("realtime-connection-label"),
                        {connection_message}
                    }
                    if can_manual_refresh {
                        button {
                            class: "public-connection-refresh",
                            r#type: "button",
                            onclick: move |_| {
                                refresh_generation.set(refresh_generation().saturating_add(1));
                            },
                            {t!("realtime-refresh")}
                        }
                    }
                    session::SessionMenu {}
                    Link {
                        class: "public-shell__utility-link public-shell__utility-link--status",
                        to: Route::Status {},
                        {t!("nav-status")}
                    }
                    LanguagePicker { label: t!("language-label") }
                }
            }
            main { class: "public-shell__content",
                Outlet::<Route> {}
            }
        }
    }
}
