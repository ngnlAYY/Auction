//! Public Dioxus 路由、语言初始化和页面外壳。

use super::*;

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css", AssetOptions::css().with_static_head(true));

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

        #[route("/login?:return_to")]
        Login { return_to: Option<String> },

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
    // Router 会规范化未声明的 query；在它挂载前分别保留页面查询状态和认证返回来源。
    let initial_location = initial_public_location();
    let provided_initial_location = initial_location.clone();
    use_context_provider(move || provided_initial_location);
    use_effect(move || initial_location.expire());
    use_context_provider(initial_login_return_location);
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
        Router::<Route> {}
    }
}

/// 公开端外壳：沿用旧版 Auction-BF 的单行页头、导航和居中内容区。
#[component]
pub(crate) fn PublicShell() -> Element {
    use_dialog_focus_trap(true);
    let selected_world = use_signal(|| selected_world_from_query(read_public_query().world));
    let location_generation = use_signal(|| 0_u64);
    let mut refresh_generation = use_signal(|| 0_u64);
    let realtime_state = use_signal(|| LiveConnectionState::Connecting);
    let readiness = use_resource(fetch_public_readiness);
    let session = use_resource(fetch_public_session);
    let current_auction = use_resource(move || {
        let _ = refresh_generation();
        let world = selected_world();
        async move {
            let result = fetch_current_auction(world.clone()).await;
            (world, result)
        }
    });
    use_context_provider(|| selected_world);
    use_context_provider(|| refresh_generation);
    use_context_provider(|| realtime_state);
    let session_context = session;
    use_context_provider(move || session_context);
    let current_auction_context = current_auction;
    use_context_provider(move || current_auction_context);

    #[cfg(target_arch = "wasm32")]
    use_effect({
        let mut location_generation = location_generation;
        move || {
            let Some(window) = web_sys::window() else {
                return;
            };
            let callback = Closure::<dyn FnMut()>::new(move || {
                let world = selected_world_from_query(read_browser_public_query().world);
                let mut selected_world = selected_world;
                selected_world.set(world);
                location_generation.set(location_generation().saturating_add(1));
            });
            let _ = window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref());
            // PublicShell 与页面同寿命；监听器在整个 SPA 生命周期内只安装一次。
            callback.forget();
        }
    });
    let location_generation = location_generation();
    // Dioxus 的 key 特殊属性会消费该值，但 WASM release 展开后仍会产生误报；显式读取保留门禁整洁。
    let _ = location_generation;

    let readiness_state = match &*readiness.read_unchecked() {
        None => PublicReadinessState::Checking,
        Some(Ok(true)) => PublicReadinessState::Ready,
        Some(Ok(false) | Err(_)) => PublicReadinessState::Degraded,
    };
    let session_snapshot = session.read().clone();
    let session_pending = session_snapshot.is_none();
    let authenticated = matches!(
        &session_snapshot,
        Some(Ok(Some(identity))) if identity.authenticated
    );
    let selected_world_value = selected_world();
    let realtime_target = match session_snapshot {
        None => PublicRealtimeTarget::Checking,
        Some(Err(_)) => PublicRealtimeTarget::SessionError,
        Some(Ok(Some(identity))) if identity.authenticated => {
            match current_auction.read().clone() {
                Some((loaded_world, Ok(Some(auction)))) if loaded_world == selected_world_value => {
                    PublicRealtimeTarget::Auction {
                        auction_id: auction.id,
                        world: auction.world,
                        preview_revision: auction.preview_revision,
                    }
                }
                Some((loaded_world, _)) if loaded_world == selected_world_value => {
                    PublicRealtimeTarget::Discovery
                }
                _ => PublicRealtimeTarget::Checking,
            }
        }
        Some(Ok(None)) | Some(Ok(Some(_))) => PublicRealtimeTarget::Anonymous,
    };
    let realtime_key = realtime_target.key();
    // Dioxus 的 key 特殊属性会消费该值，但 WASM release 展开后仍会产生误报。
    let _ = realtime_key.as_str();
    let has_current_auction = matches!(&realtime_target, PublicRealtimeTarget::Auction { .. });
    let connection_state = public_connection_state(
        readiness_state,
        session_pending,
        authenticated,
        has_current_auction,
        realtime_state(),
    );
    let connection_class = public_connection_class(connection_state);
    let connection_state_name = public_connection_state_name(connection_state);
    let connection_message = realtime::public_connection_message(connection_state);
    let can_manual_refresh =
        matches!(connection_state, PublicConnectionState::Degraded | PublicConnectionState::Manual);

    rsx! {
        div { class: "public-shell",
            PublicRealtimeOwner {
                key: realtime_key,
                target: realtime_target,
                refresh_generation,
                realtime_state,
            }
            a { class: "public-shell__skip", href: "#public-content", {t!("skip")} }
            header { class: "public-shell__header",
                Link { class: "public-shell__logo", to: Route::AuctionIndex {},
                    {t!("app-title")}
                }
                nav { class: "public-shell__nav", aria_label: t!("nav-label"),
                    Link { to: Route::AuctionIndex {}, {t!("nav-auctions")} }
                    Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
                    Link { to: Route::AuctionList {}, {t!("nav-auction-history")} }
                    Link { to: Route::History {}, {t!("nav-price-history")} }
                    Link { to: Route::Rules {}, {t!("page-rules-title")} }
                    Link { to: Route::Blacklist {}, {t!("nav-blacklist")} }
                    Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
                }
                div { class: "public-shell__controls",
                    WorldPicker { selected_world }
                    span {
                        class: "public-connection-badge {connection_class}",
                        "data-state": connection_state_name,
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
                    LanguagePicker {
                        label: t!("language-label"),
                        default_locale: "en",
                    }
                    session::SessionMenu {}
                }
            }
            main { id: "public-content", class: "public-shell__content",
                Outlet::<Route> { key: location_generation.to_string() }
            }
        }
    }
}

/// 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main，地址栏保持原值。
fn selected_world_from_query(value: Option<String>) -> String {
    if value.as_deref() == Some("isekai") { "isekai" } else { "main" }.to_owned()
}
