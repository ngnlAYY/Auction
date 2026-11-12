//! Public Dioxus 路由、语言初始化和页面外壳。

use super::*;

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
    if !session.is_ready() {
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
    // world 同步覆盖 Router Link push、同路径 query 导航和浏览器前进/后退。
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
    let mut sidebar_collapsed = use_signal(load_sidebar_collapsed);
    let mut refresh_generation = use_signal(|| 0_u64);
    let mut manual_refresh_busy = use_signal(|| false);
    let mut manual_refresh_error = use_signal(|| false);
    let bid_refresh = PublicBidRefresh(use_signal(|| 0_u64));
    let mut readiness_generation = use_signal(|| 0_u64);
    // 游标归整个 Public 外壳；订阅因目标或用户身份变化重启时仍保留已确认位置。
    let discovery_after = use_signal(|| 0_i64);
    let auction_after = use_signal(|| None::<(String, i64)>);
    let mut stable_realtime_target = use_signal(|| PublicRealtimeTarget::Checking);
    let page_realtime_target = use_signal(|| None::<(PublicRealtimePageKey, PublicRealtimeTarget)>);
    let realtime_state = use_signal(|| LiveConnectionState::Connecting);
    let readiness = use_resource(move || {
        let _ = readiness_generation();
        fetch_public_readiness()
    });
    let mut session = use_resource(fetch_public_session);
    // 满额登录不再拒绝，而是由服务端注销最久未活动的会话；被注销的设备在这里看到提示。
    let mut session_ended_elsewhere = use_signal(|| false);
    let mut command_owner = use_signal(|| None::<String>);
    use_effect(move || {
        if let Some(Ok(identity)) = session.read().as_ref() {
            let next = identity
                .as_ref()
                .filter(|identity| identity.authenticated)
                .map(|identity| identity.user_id.clone());
            if *command_owner.peek() != next {
                command_owner.set(next);
            }
            if identity.as_ref().is_some_and(|identity| identity.authenticated) {
                // 重新登录成功后提示自动失效；不保留旧提示继续指向已恢复会话语义。
                session_ended_elsewhere.set(false);
            }
        }
    });
    let check_session = use_callback(move |reason: SessionRefreshReason| {
        if reason.invalidates() {
            // 未到期却失效的认证会话只可能来自服务端撤销：满额登录腾出的名额、换钥或管理员撤销。
            // 该设备据此看到“已在其他设备登录”，用原登录 key 重新登录即可恢复。
            if reason.shows_ended_elsewhere()
                && session.read().as_ref().is_some_and(|result| {
                    result.as_ref().is_ok_and(|identity| {
                        identity.as_ref().is_some_and(session_ended_by_revocation)
                    })
                })
            {
                session_ended_elsewhere.set(true);
            }
            // 已知认证变化先卸载旧用户命令；普通 focus 重读保留原快照，避免打断同身份提交。
            command_owner.set(None);
            session.set(None);
        }
        session.restart();
    });
    use_context_provider(|| crate::api::PublicSessionCheckTrigger(check_session));
    super::session::use_public_session_sync(check_session);
    super::session::use_public_session_expiry(session, check_session);
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
                check_session.call(SessionRefreshReason::Refresh);
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
    use_context_provider(|| PublicPageRealtimeTarget(page_realtime_target));
    use_context_provider(|| PublicLocationRevision(location_generation));
    let session_context = session;
    use_context_provider(move || session_context);
    use_context_provider(|| PublicCommandOwner(command_owner));
    let readiness_context = readiness;
    use_context_provider(move || readiness_context);
    let current_auction_context = current_auction;
    use_context_provider(move || current_auction_context);
    use_context_provider(move || page_visibility);

    // world 键的恢复已由 query hook 承担；这里只为查询驱动页面（拍卖看板的历史/首页切换）
    // 递增位置版本。
    auction_web_shared::use_popstate_revision(location_generation);
    let readiness_state = match &*readiness.read_unchecked() {
        None => PublicReadinessState::Checking,
        Some(Ok(true)) => PublicReadinessState::Ready,
        Some(Ok(false) | Err(_)) => PublicReadinessState::Degraded,
    };
    let session_snapshot = session.read();
    let session_view = PublicSessionView::from_snapshot(session_snapshot.as_ref());
    let session_pending = session_view.pending;
    let authenticated = session_view.authenticated;
    let realtime_user_id = session_snapshot
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(Option::as_ref)
        .filter(|identity| identity.authenticated)
        .map(|identity| identity.user_id.clone());
    let selected_world_value = selected_world();
    let current_public_query = read_public_query();
    let auction_home_query = public_auction_home_query(&selected_world_value);
    let auction_home_href = public_auction_home_href(&selected_world_value);
    // Routable 不包含 query；位于 AuctionIndex 时，只有查询不同才需要绕过 Router 的同路由优化。
    let auction_home_needs_query_reset = matches!(&current_route, Route::AuctionIndex {})
        && current_public_query != auction_home_query;
    let requested_page_target = match &current_route {
        Route::LotDetail { lot_code } => Some(PublicRealtimePageKey::Lot(lot_code.clone())),
        Route::AuctionIndex {} | Route::AuctionPage {} => {
            current_public_query.auction_id.clone().map(PublicRealtimePageKey::Auction)
        }
        _ => None,
    };
    let realtime_target_candidate =
        match (session_snapshot.as_ref(), requested_page_target.as_ref()) {
            (None, _) => PublicRealtimeTarget::Checking,
            (Some(Err(_)), _) => PublicRealtimeTarget::SessionError,
            (Some(Ok(_)), Some(requested)) => match page_realtime_target.read().as_ref() {
                Some((loaded, target)) if loaded == requested => {
                    if matches!(target, PublicRealtimeTarget::Auction { .. }) && !authenticated {
                        PublicRealtimeTarget::Anonymous
                    } else {
                        target.clone()
                    }
                }
                _ => PublicRealtimeTarget::Checking,
            },
            (Some(Ok(_)), None) => match current_auction.read().as_ref() {
                Some((loaded_world, Ok(Some(auction))))
                    if loaded_world == &selected_world_value
                        && matches!(auction.status.as_str(), "live" | "final")
                        && authenticated =>
                {
                    PublicRealtimeTarget::Auction {
                        auction_id: auction.id.clone(),
                        world: auction.world.clone(),
                        preview_revision: auction.preview_revision,
                    }
                }
                Some((loaded_world, Ok(Some(auction))))
                    if loaded_world == &selected_world_value =>
                {
                    if matches!(auction.status.as_str(), "live" | "final") {
                        PublicRealtimeTarget::Anonymous
                    } else {
                        PublicRealtimeTarget::Inactive {
                            world: selected_world_value.clone(),
                            preview: auction.status == "preview",
                        }
                    }
                }
                Some((loaded_world, Ok(None))) if loaded_world == &selected_world_value => {
                    PublicRealtimeTarget::Inactive {
                        world: selected_world_value.clone(),
                        preview: false,
                    }
                }
                Some((loaded_world, Err(_))) if loaded_world == &selected_world_value => {
                    PublicRealtimeTarget::CurrentError { world: selected_world_value.clone() }
                }
                _ => PublicRealtimeTarget::Checking,
            },
        };
    // 同一 world 的权威 current 快照重读期间保留既有目标；首屏、登出和 world 切换仍失败关闭。
    let remembered_target = stable_realtime_target();
    let realtime_target = match realtime_target_candidate {
        PublicRealtimeTarget::Checking if authenticated && requested_page_target.is_none() => {
            match &remembered_target {
                PublicRealtimeTarget::Auction { world, .. } if world == &selected_world_value => {
                    remembered_target
                }
                PublicRealtimeTarget::Inactive { world, .. } if world == &selected_world_value => {
                    remembered_target
                }
                _ => PublicRealtimeTarget::Checking,
            }
        }
        target => target,
    };
    use_effect(use_reactive((&realtime_target,), move |(remembered,)| {
        if *stable_realtime_target.peek() != remembered {
            stable_realtime_target.set(remembered);
        }
    }));
    let inactive = matches!(&realtime_target, PublicRealtimeTarget::Inactive { .. });
    let preview = matches!(&realtime_target, PublicRealtimeTarget::Inactive { preview: true, .. });
    let connection_state = public_connection_state(
        readiness_state,
        session_pending,
        authenticated,
        &realtime_target,
        realtime_state(),
    );
    let connection_class = public_connection_class(connection_state);
    let connection_state_name = public_connection_state_name(connection_state);
    let connection_message = if preview && connection_state == PublicConnectionState::Idle {
        t!("auction-status-preview")
    } else {
        realtime::public_connection_message(connection_state)
    };
    let connection_label =
        if inactive { connection_message.clone() } else { t!("realtime-connection-label") };
    let connection_title = connection_message.clone();
    let can_manual_refresh =
        matches!(connection_state, PublicConnectionState::Degraded | PublicConnectionState::Manual);

    let go_home = EventHandler::new(move |()| {
        if auction_home_needs_query_reset {
            // 同路由的首页入口仍清除历史场次与筛选；两端导航共享同一写回动作。
            let query = public_auction_home_query(&selected_world.peek());
            set_public_query(&query, false);
        }
        navigation_open.set(false);
        location_generation.set(location_generation().saturating_add(1));
    });

    let ticker_auction = {
        let snapshot = current_auction.read();
        snapshot.as_ref().and_then(|(world, result)| {
            (world == &selected_world_value)
                .then(|| result.as_ref().ok().cloned().flatten())
                .flatten()
        })
    };
    let ticker_title = ticker_auction
        .as_ref()
        .map(|auction| t!("auction-header-title", number: auction.number))
        .unwrap_or_else(|| t!("nav-auctions"));
    let ticker_status = ticker_auction
        .as_ref()
        .map(|auction| auction_status_label(&auction.status))
        .unwrap_or_default();
    let ticker_world = ticker_auction
        .as_ref()
        .map(|auction| world_label(&auction.world))
        .unwrap_or_else(|| world_label(&selected_world_value));
    let ticker_ends_at =
        ticker_auction.as_ref().and_then(|auction| auction.scheduled_ends_at.clone());

    rsx! {
        div {
            class: if sidebar_collapsed() {
                "public-shell public-console is-sidebar-collapsed"
            } else {
                "public-shell public-console"
            },
            PublicRealtimeOwner {
                user_id: realtime_user_id,
                target: realtime_target,
                refresh_generation,
                discovery_after,
                auction_after,
                realtime_state,
            }
            a { class: "console-skip", href: "#public-content", {t!("skip")} }
            header { class: "console-ticker",
                div { class: "console-ticker__left",
                    presentation::mobile::Navigation {
                        auction_home_href,
                        reset_home_query: auction_home_needs_query_reset,
                        open: navigation_open(),
                        on_toggle: move |_| navigation_open.set(!navigation_open()),
                        on_close: move |_| navigation_open.set(false),
                        on_home: go_home,
                    }
                    if !ticker_status.is_empty() {
                        span { class: "console-ticker__dot", aria_hidden: "true" }
                        span { class: "console-ticker__live", "{ticker_status}" }
                        span { class: "console-ticker__sep", aria_hidden: "true" }
                    }
                    span { class: "console-ticker__auction", "{ticker_title}" }
                    span { class: "console-ticker__sep", aria_hidden: "true" }
                    span { class: "console-ticker__label", "{ticker_world}" }
                    if let Some(ends_at) = ticker_ends_at.clone() {
                        span { class: "console-ticker__sep", aria_hidden: "true" }
                        span { class: "console-ticker__label", {t!("auction-header-ends-in")} }
                        presentation::console::ConsoleCountdown { ends_at }
                    }
                }
                div { class: "console-ticker__right",
                    div { class: "console-ticker__connection",
                        div { class: "public-shell__connection",
                            span {
                                class: "public-connection-badge {connection_class}",
                                "data-state": connection_state_name,
                                role: "status",
                                aria_label: connection_label,
                                title: connection_title,
                                span { class: "public-connection-badge__label", {connection_message} }
                            }
                            if can_manual_refresh || inactive {
                                button {
                                    class: "public-connection-refresh",
                                    r#type: "button",
                                    disabled: manual_refresh_busy(),
                                    aria_busy: manual_refresh_busy().to_string(),
                                    onclick: move |_| {
                                        if *manual_refresh_busy.peek() {
                                            return;
                                        }
                                        manual_refresh_error.set(false);
                                        manual_refresh_busy.set(true);
                                        spawn(async move {
                                            gloo_timers::future::TimeoutFuture::new(80).await;
                                            let reloaded = web_sys::window()
                                                .is_some_and(|window| window.location().reload().is_ok());
                                            if !reloaded {
                                                manual_refresh_busy.set(false);
                                                manual_refresh_error.set(true);
                                                let next_refresh = refresh_generation.peek().saturating_add(1);
                                                refresh_generation.set(next_refresh);
                                            }
                                        });
                                    },
                                    if manual_refresh_busy() {
                                        span { class: "public-connection-refresh__spinner", aria_hidden: "true", "↻" }
                                        {t!("realtime-refreshing")}
                                    } else {
                                        {t!("realtime-refresh")}
                                    }
                                }
                            }
                            if manual_refresh_error() {
                                span { class: "public-connection-refresh__error", role: "alert", {t!("data-error")} }
                            }
                        }
                    }
                    div { class: "public-shell__controls",
                        div { class: "public-shell__preferences",
                            WorldPicker { selected_world }
                            LanguagePicker {
                                label: t!("language-label"),
                                catalog: &PUBLIC_LOCALE_CATALOG,
                            }
                            ThemePicker {
                                label: t!("theme-label"),
                                dark_label: t!("theme-dark"),
                                light_label: t!("theme-light"),
                            }
                        }
                    }
                    div { class: "console-ticker__session", session::SessionMenu {} }
                }
            }
            if session_ended_elsewhere() {
                div { class: "public-shell__session-ended-banner", role: "status",
                    span { class: "public-shell__session-ended-text",
                        {t!("session-ended-elsewhere")}
                    }
                    Link {
                        class: "public-shell__session-ended-login",
                        to: "/login",
                        {t!("session-ended-relogin")}
                    }
                    button {
                        class: "public-shell__session-ended-dismiss",
                        r#type: "button",
                        aria_label: t!("catalog-close"),
                        onclick: move |_| session_ended_elsewhere.set(false),
                        "✕"
                    }
                }
            }
            div { class: "console-body",
                presentation::console::ConsoleSidebar {
                    current_route: current_route.clone(),
                    collapsed: sidebar_collapsed(),
                    on_toggle: move |_| {
                        let next = !sidebar_collapsed();
                        sidebar_collapsed.set(next);
                        save_sidebar_collapsed(next);
                    },
                    on_navigate: move |_| navigation_open.set(false),
                }
                main {
                    id: "public-content",
                    class: "console-main",
                    tabindex: "-1",
                    Outlet::<Route> {}
                }
            }
        }
    }
}

/// 未到期却失效的认证会话只来自服务端撤销；自然到期不显示“已在其他设备登录”。
fn session_ended_by_revocation(identity: &PublicSession) -> bool {
    if !identity.authenticated {
        return false;
    }
    // 永久会话没有截止字段，不可能自然到期。
    let Some(expires_at) = identity.expires_at.as_deref() else {
        return true;
    };
    // 截止字符串不可解析时保守地不提示，避免把未知状态当作撤销。
    chrono::DateTime::parse_from_rfc3339(expires_at)
        .map(|expiry| expiry.with_timezone(&chrono::Utc) > estimated_now())
        .unwrap_or(false)
}

/// 旧版只把显式 Isekai 视为第二世界；缺失、Main 与非法 query 都在内存回退 Main，地址栏保持原值。
fn selected_world_from_query(value: Option<String>) -> String {
    if value.as_deref() == Some("isekai") { "isekai" } else { "main" }.to_owned()
}
