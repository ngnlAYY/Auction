//! Public Dioxus 路由、语言初始化和页面外壳。

use super::*;

/// 公开端 surface 样式。设计变量由 `auction-web-shared` 提供。
const PUBLIC_CSS: Asset = asset!("/assets/public.css", AssetOptions::css().with_static_head(true));

pub fn run() {
    dioxus::launch(App);
}

/// 公开端路由表。
///
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
        div { class: "app-root",
            Router::<Route> {}
        }
    }
}

/// 公开端外壳：沿用旧版 Auction-BF 的单行页头、导航和居中内容区。
#[component]
pub(crate) fn PublicShell() -> Element {
    use_dialog_focus_trap(true);
    let current_route = use_route::<Route>();
    let auction_page_owns_initial_boot =
        matches!(&current_route, Route::AuctionIndex {} | Route::AuctionPage {});
    use_route_focus(format!("{current_route:?}"), "public-content");
    let selected_world = use_signal(|| selected_world_from_query(read_public_query().world));
    let mut location_generation = use_signal(|| 0_u64);
    let mut navigation_open = use_signal(|| false);
    let mut refresh_generation = use_signal(|| 0_u64);
    let mut readiness_generation = use_signal(|| 0_u64);
    // 发现流游标属于整个 Public 外壳；current-auction 刷新导致实时 owner 按 key 重建时也不能丢失。
    let discovery_after = use_signal(|| 0_i64);
    let auction_after = use_signal(|| None::<(String, i64)>);
    let mut stable_realtime_target = use_signal(|| PublicRealtimeTarget::Checking);
    let realtime_state = use_signal(|| LiveConnectionState::Connecting);
    let readiness = use_resource(move || {
        let _ = readiness_generation();
        fetch_public_readiness()
    });
    let session = use_resource(fetch_public_session);
    let current_auction = use_resource(move || {
        let _ = refresh_generation();
        let world = selected_world();
        async move {
            let result = fetch_current_auction(world.clone()).await;
            (world, result)
        }
    });
    let _readiness_poll = use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(10_000).await;
            readiness_generation.set(readiness_generation().saturating_add(1));
        }
    });
    let _authoritative_current_poll = use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(60_000).await;
            refresh_generation.set(refresh_generation().saturating_add(1));
        }
    });
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    // 默认拍卖页还要等待首屏 lots；其他路由在三项外壳请求都有结果后即可接管静态骨架。
    use_app_boot_ready(boot_prerequisites_ready && !auction_page_owns_initial_boot);
    use_context_provider(|| selected_world);
    use_context_provider(|| refresh_generation);
    use_context_provider(|| realtime_state);
    use_context_provider(|| PublicLocationRevision(location_generation));
    let session_context = session;
    use_context_provider(move || session_context);
    let readiness_context = readiness;
    use_context_provider(move || readiness_context);
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
    let current_public_query = read_public_query();
    let auction_home_query = public_auction_home_query(&selected_world_value);
    let auction_home_href = public_auction_home_href(&selected_world_value);
    // Routable 不包含 query；位于 AuctionIndex 时，只有查询不同才需要绕过 Router 的同路由优化。
    let auction_home_needs_query_reset = matches!(&current_route, Route::AuctionIndex {})
        && current_public_query != auction_home_query;
    let logo_home_query = auction_home_query.clone();
    let realtime_target_candidate = match session_snapshot {
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
    // 同一 world 的权威 current 快照重读期间保留既有目标；首屏、登出和 world 切换仍失败关闭。
    let remembered_target = stable_realtime_target();
    let realtime_target = match realtime_target_candidate {
        PublicRealtimeTarget::Checking if authenticated => match &remembered_target {
            PublicRealtimeTarget::Auction { world, .. } if world == &selected_world_value => {
                remembered_target
            }
            PublicRealtimeTarget::Discovery => remembered_target,
            _ => PublicRealtimeTarget::Checking,
        },
        target => target,
    };
    use_effect({
        let remembered = realtime_target.clone();
        move || {
            if stable_realtime_target.peek().clone() != remembered {
                stable_realtime_target.set(remembered.clone());
            }
        }
    });
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
                discovery_after,
                auction_after,
                realtime_state,
            }
            a { class: "public-shell__skip", href: "#public-content", {t!("skip")} }
            header { class: "public-shell__header",
                Link {
                    class: "public-shell__logo",
                    to: auction_home_href.clone(),
                    onclick_only: auction_home_needs_query_reset,
                    onclick: move |_| {
                        if auction_home_needs_query_reset {
                            // 同一路由时直接写入首页查询，否则 Router 可能保留当前历史拍卖 URL。
                            set_public_query(&logo_home_query, false);
                        }
                        location_generation
                            .set(location_generation().saturating_add(1));
                    },
                    {t!("app-title")}
                }
                button {
                    class: "public-shell__nav-toggle",
                    r#type: "button",
                    aria_controls: "public-primary-navigation",
                    aria_expanded: navigation_open().to_string(),
                    onclick: move |_| navigation_open.set(!navigation_open()),
                    {t!("nav-label")}
                }
                nav {
                    id: "public-primary-navigation",
                    class: if navigation_open() {
                        "public-shell__nav public-shell__nav--open"
                    } else {
                        "public-shell__nav"
                    },
                    aria_label: t!("nav-label"),
                    onclick: move |_| navigation_open.set(false),
                    Link {
                        to: auction_home_href,
                        onclick_only: auction_home_needs_query_reset,
                        onclick: move |_| {
                            if auction_home_needs_query_reset {
                                // 清除 auction、筛选和分页查询，再重建共享 AuctionIndex 的内容。
                                set_public_query(&auction_home_query, false);
                            }
                            location_generation
                                .set(location_generation().saturating_add(1));
                        },
                        {t!("nav-auctions")}
                    }
                    Link { to: Route::BidLog {}, {t!("page-bidlog-title")} }
                    Link { to: Route::AuctionList {}, {t!("nav-auction-history")} }
                    Link { to: Route::History {}, {t!("nav-price-history")} }
                    Link { to: Route::Rules {}, {t!("page-rules-title")} }
                    Link { to: Route::Blacklist {}, {t!("nav-blacklist")} }
                    Link { to: Route::CompensationPool {}, {t!("page-compensation-pool-title")} }
                    Link { to: Route::Forum {}, {t!("nav-forum")} }
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
            main { id: "public-content", class: "public-shell__content", tabindex: "-1",
                Outlet::<Route> {}
            }
        }
    }
}

/// 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main，地址栏保持原值。
fn selected_world_from_query(value: Option<String>) -> String {
    if value.as_deref() == Some("isekai") { "isekai" } else { "main" }.to_owned()
}
