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
    // 秒级共享时钟在外壳根提供；看板与 lot 详情的倒计时通过 use_second_tick 订阅同一节拍。
    use_second_ticker();
    let current_route = use_route::<Route>();
    let auction_page_owns_initial_boot =
        matches!(&current_route, Route::AuctionIndex {} | Route::AuctionPage {});
    use_route_focus(format!("{current_route:?}"), "public-content");
    // world 键的浏览器前进/后退恢复由共享 query hook 承担；signal 仍是页头选择器与
    // 外壳资源的写入点，hook 状态经下面的 effect 单向同步进来。
    let world_query_state = use_public_query_state();
    let world_query_signal = world_query_state.state;
    let mut selected_world = use_signal(|| selected_world_from_query(read_public_query().world));
    use_effect(move || {
        let world = selected_world_from_query(world_query_signal().world.clone());
        if *selected_world.peek() != world {
            selected_world.set(world);
        }
    });
    let mut location_generation = use_signal(|| 0_u64);
    let mut navigation_open = use_signal(|| false);
    let mut refresh_generation = use_signal(|| 0_u64);
    let bid_refresh = PublicBidRefresh(use_signal(|| 0_u64));
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
    // 页面可见性在轮询与实时重连退避之间共享；隐藏时全部挂起（不重置周期），
    // 恢复可见时立即补偿一次刷新并恢复各自节奏。
    let page_visibility = use_hook(PageVisibility::new);
    let readiness_poll_visibility = page_visibility.clone();
    let _readiness_poll = use_future(move || {
        let page_visibility = readiness_poll_visibility.clone();
        async move {
            loop {
                sleep_while_visible(&page_visibility, 10_000).await;
                readiness_generation.set(readiness_generation().saturating_add(1));
            }
        }
    });
    let current_poll_visibility = page_visibility.clone();
    let _authoritative_current_poll = use_future(move || {
        let page_visibility = current_poll_visibility.clone();
        async move {
            loop {
                sleep_while_visible(&page_visibility, 60_000).await;
                refresh_generation.set(refresh_generation().saturating_add(1));
            }
        }
    });
    #[cfg(target_arch = "wasm32")]
    use_effect({
        let page_visibility = page_visibility.clone();
        move || {
            // 恢复可见时立即补偿一次 readiness 与权威 current 重读，快速回到同步。
            page_visibility.install(move || {
                readiness_generation.set(readiness_generation().saturating_add(1));
                refresh_generation.set(refresh_generation().saturating_add(1));
            });
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        let cleanup_visibility = page_visibility.clone();
        use_drop(move || cleanup_visibility.cleanup());
    }
    let boot_prerequisites_ready =
        readiness.read().is_some() && session.read().is_some() && current_auction.read().is_some();
    // 默认拍卖页还要等待首屏 lots；其他路由在三项外壳请求都有结果后即可接管静态骨架。
    use_app_boot_ready(boot_prerequisites_ready && !auction_page_owns_initial_boot);
    use_context_provider(|| selected_world);
    // 页头 WorldPicker 的 URL 写回经外壳 hook 状态信号，保证 popstate 恢复的相等判断不失效。
    use_context_provider(|| world_query_signal);
    use_context_provider(|| refresh_generation);
    use_context_provider(|| bid_refresh);
    use_context_provider(|| realtime_state);
    use_context_provider(|| PublicLocationRevision(location_generation));
    let session_context = session;
    use_context_provider(move || session_context);
    let readiness_context = readiness;
    use_context_provider(move || readiness_context);
    let current_auction_context = current_auction;
    use_context_provider(move || current_auction_context);
    use_context_provider(move || page_visibility);

    #[cfg(target_arch = "wasm32")]
    let popstate_guard = use_hook(|| Rc::new(RefCell::new(None::<PublicPopstateGuard>)));
    #[cfg(target_arch = "wasm32")]
    {
        let cleanup_guard = popstate_guard.clone();
        use_drop(move || {
            if let Some(guard) = cleanup_guard.borrow_mut().take() {
                guard.cleanup();
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    use_effect({
        let mut location_generation = location_generation;
        let guard = popstate_guard.clone();
        move || {
            if guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let callback = Closure::<dyn FnMut()>::new(move || {
                // world 键的恢复已由 query hook 承担；这里只保留查询驱动页面（拍卖看板的
                // 历史/首页切换）需要的位置版本递增。
                location_generation.set(location_generation().saturating_add(1));
            });
            let _ = window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref());
            *guard.borrow_mut() = Some(PublicPopstateGuard { window, callback });
        }
    });
    let readiness_state = match &*readiness.read_unchecked() {
        None => PublicReadinessState::Checking,
        Some(Ok(true)) => PublicReadinessState::Ready,
        Some(Ok(false) | Err(_)) => PublicReadinessState::Degraded,
    };
    let session_snapshot = session.read().clone();
    let session_view = PublicSessionView::from_snapshot(session_snapshot.as_ref());
    let session_pending = session_view.pending;
    let authenticated = session_view.authenticated;
    let selected_world_value = selected_world();
    let current_public_query = read_public_query();
    let auction_home_query = public_auction_home_query(&selected_world_value);
    let auction_home_href = public_auction_home_href(&selected_world_value);
    // Routable 不包含 query；位于 AuctionIndex 时，只有查询不同才需要绕过 Router 的同路由优化。
    let auction_home_needs_query_reset = matches!(&current_route, Route::AuctionIndex {})
        && current_public_query != auction_home_query;
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
                            let query = public_auction_home_query(&selected_world.peek());
                            set_public_query(&query, false);
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
                                let query = public_auction_home_query(&selected_world.peek());
                                set_public_query(&query, false);
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

#[cfg(target_arch = "wasm32")]
struct PublicPopstateGuard {
    window: web_sys::Window,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl PublicPopstateGuard {
    fn cleanup(self) {
        let _ = self.window.remove_event_listener_with_callback(
            "popstate",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}

/// 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main，地址栏保持原值。
fn selected_world_from_query(value: Option<String>) -> String {
    if value.as_deref() == Some("isekai") { "isekai" } else { "main" }.to_owned()
}
