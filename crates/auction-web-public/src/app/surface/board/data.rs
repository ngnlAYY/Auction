//! Public 拍卖数据加载、倒计时和目录展示编排。

use super::*;

#[component]
pub(crate) fn AuctionData(
    auction: CurrentAuction,
    historical: bool,
    boot_prerequisites_ready: bool,
) -> Element {
    // 开场刷新每秒重算：订阅外壳提供的共享秒级节拍，不再自起独立定时器。
    // 倒计时显示的秒针订阅已下沉到 AuctionHeaderCountdown / LiveRowCountdown 叶子组件，
    // 顶层不再持有每秒变化的时钟信号，避免整个 AuctionData 子树每秒全量重渲染。
    let tick = use_second_tick();
    // 分类与页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    // Signal 为 Copy，两个 memo 共享同一状态信号，只在各自字段变化时通知订阅者。
    let query_state_signal = query_state.state;
    let active_category =
        use_memo(move || query_state_signal().category.clone().unwrap_or_else(|| "all".to_owned()));
    let page_number = use_memo(move || query_state_signal().page.max(1));
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let should_refresh_at_open = !historical && auction.presentation_mode != "live";
    let opening_at =
        DateTime::parse_from_rfc3339(&auction.opens_at).ok().map(|value| value.with_timezone(&Utc));
    let opening_key = format!("{}:{}", auction.id, auction.opens_at);
    let mut opening_refresh_key = use_signal(|| None::<String>);
    let mut opening_refreshed = use_signal(|| false);
    use_effect(move || {
        let _ = tick();
        let now = estimated_now();
        if *opening_refresh_key.peek() != Some(opening_key.clone()) {
            opening_refresh_key.set(Some(opening_key.clone()));
            opening_refreshed.set(false);
        }
        if should_refresh_at_open
            && !*opening_refreshed.peek()
            && *realtime_state.peek() == LiveConnectionState::Live
            && opening_at.is_some_and(|opens_at| now >= opens_at)
        {
            opening_refreshed.set(true);
            let next_generation = refresh_generation.peek().saturating_add(1);
            refresh_generation.set(next_generation);
        }
    });
    let auction_id = auction.id.clone();
    let mut session = use_context::<PublicSessionResource>();
    let session_view = PublicSessionView::from_snapshot(session.read().as_ref());
    let viewer_player_id = session_view.viewer_player_id.clone();
    let session_ready = session_view.ready;
    let session_error = session_view.error;
    let authenticated = session_view.authenticated;
    let history_switch = use_resource({
        let world = auction.world.clone();
        let should_load = historical || auction.presentation_mode != "live";
        move || {
            let world = world.clone();
            async move {
                if !should_load {
                    return Ok(None);
                }
                let current = fetch_current_auction(world).await?;
                let rounds = fetch_auction_list().await?;
                Ok(Some((current, rounds)))
            }
        }
    });
    let mut lots_refresh_generation = use_signal(|| 0_u64);
    let lots = use_resource(move || {
        let _ = refresh_generation();
        let _ = lots_refresh_generation();
        let auction_id = auction_id.clone();
        let category = active_category();
        let page = page_number();
        let category = (category != "all").then_some(category);
        async move { fetch_current_lots(auction_id, category, page).await }
    });
    use_bid_refresh(lots, true);
    // 目录行是 lots 快照的纯派生：用 memo 收敛为仅在资源结果变化时重建，
    // 并以 Rc 共享给分组/表格，传参链路不再逐字段克隆整表。
    let rows = use_memo(move || match &*lots.read() {
        Some(Ok(page)) => Rc::new(page.rows()),
        _ => Rc::new(Vec::new()),
    });
    let aligned = match &*lots.read_unchecked() {
        Some(Ok(page)) => {
            page.auction_id == auction.id
                && page.auction_status == auction.status
                && page.presentation_mode == auction.presentation_mode
                && page.bidding_open == auction.bidding_open
                && page.preview_revision == auction.preview_revision
        }
        _ => true,
    };
    // 首次揭示要等主要目录已有成功或失败结果；辅助视图切换继续局部异步，避免拖慢整页。
    use_app_boot_ready(boot_prerequisites_ready && lots.read().is_some());
    let alignment_key = format!(
        "{}:{}:{}:{}:{}",
        auction.id,
        auction.status,
        auction.presentation_mode,
        auction.bidding_open,
        auction.preview_revision
    );
    let mut alignment_attempt = use_signal(|| None::<String>);
    use_effect(move || {
        if aligned {
            if alignment_attempt().is_some() {
                alignment_attempt.set(None);
            }
        } else if alignment_attempt().as_deref() != Some(alignment_key.as_str()) {
            alignment_attempt.set(Some(alignment_key.clone()));
            lots_refresh_generation.set(lots_refresh_generation().saturating_add(1));
        }
    });
    let status_label = auction_status_label(&auction.status);
    let is_live_presentation = auction.presentation_mode == "live";
    let is_opening_presentation = auction.presentation_mode == "opening";
    let show_live_countdown =
        is_live_presentation && matches!(auction.status.as_str(), "live" | "final");
    let auction_end = if show_live_countdown {
        auction.scheduled_ends_at.clone()
    } else if is_opening_presentation {
        None
    } else if !is_live_presentation {
        Some(auction.opens_at.clone())
    } else {
        // settling/settled 已进入终态收口，不再把过去的计划截止时间渲染为“落锤中…”。
        None
    };
    let catalog = match &*lots.read_unchecked() {
        None => {
            rsx! { p { class: "public-page-status", role: "status", {t!("common-loading")} } }
        }
        Some(Err(error)) => {
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("data-error"),
                    detail: lot_list_error_message(error),
                    retry_label: t!("data-retry"),
                    on_retry: move |_| {
                        lots_refresh_generation
                            .set(lots_refresh_generation().saturating_add(1));
                    },
                }
            }
        }
        Some(Ok(_)) if !aligned => rsx! {
            p { class: "public-page-status", role: "status", {t!("common-loading")} }
        },
        Some(Ok(page)) => {
            let page_size = page.page_size.filter(|size| *size > 0).unwrap_or(20);
            // 旧 lot 外壳为详情和列表共用，列表 total 可以省略且没有 has_more 字段；满页时
            // 保留一次向后探测，避免把“当前页长度”伪装成总数并永久隐藏下一页。
            let has_more = page.total.is_none()
                && i64::try_from(rows.read().len()).is_ok_and(|row_count| row_count >= page_size);
            rsx! {
                CatalogTable {
                    rows: rows(),
                    projection_kind: page.projection_kind.clone(),
                    category_counts: page.category_counts.clone().unwrap_or_default(),
                    active_category: active_category(),
                    page_number: page_number(),
                    on_category_change: move |category: String| {
                        update_public_query_state(
                            query_state_signal,
                            |query| {
                                query.category = (category != "all").then_some(category.clone());
                                query.page = 1;
                            },
                            QueryWriteMode::Push,
                        );
                    },
                    on_page_change: move |page| {
                        update_public_query_state(
                            query_state_signal,
                            |query| query.page = page,
                            QueryWriteMode::Push,
                        );
                    },
                    page_total: page.total,
                    page_size,
                    has_more,
                    can_bid: is_live_presentation && auction.bidding_open,
                    allow_slow: auction.status == "live",
                    session_ready,
                    authenticated,
                    viewer_player_id: viewer_player_id.clone(),
                }
            }
        }
    };

    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            div { class: "public-auction-live",
                header { class: "public-auction-round",
                    div { class: "public-auction-round__title",
                        h1 {
                            id: "auction-title",
                            {t!("auction-header-title", number: auction.number)}
                        }
                        span { class: "public-auction-round__world", "{auction_world_label(&auction.world)}" }
                        span {
                            class: "public-auction-round__status public-auction-round__status--{auction_status_class(&auction.status)}",
                            "{status_label}"
                        }
                    }
                    if is_opening_presentation {
                        p { class: "public-auction-round__end",
                            {t!("auction-header-opening")}
                        }
                    } else if let Some(scheduled_ends_at) = auction_end.clone() {
                        AuctionHeaderCountdown {
                            scheduled_ends_at,
                            is_live_presentation,
                            status: auction.status.clone(),
                        }
                    }
                    {preview_history_switch(&history_switch.read_unchecked(), &auction, historical)}
                }
                if !historical && is_live_presentation && auction.bidding_open && session_error {
                    div { class: "ui-empty-state", role: "alert",
                        p { class: "ui-empty-state__title", {t!("session-error")} }
                        button {
                            class: "ui-empty-state__retry",
                            r#type: "button",
                            onclick: move |_| session.restart(),
                            {t!("data-retry")}
                        }
                    }
                }
                if !historical && is_live_presentation {
                    section { aria_labelledby: "auction-lots",
                        h2 { id: "auction-lots", class: "public-visually-hidden",
                            {t!("auction-lots")}
                        }
                        {catalog}
                    }
                } else if !historical && auction.presentation_mode == "preview" {
                    section { aria_labelledby: "auction-preview-lots",
                        h2 { id: "auction-preview-lots", class: "public-visually-hidden",
                            {t!("auction-preview-lots")}
                        }
                        {catalog}
                    }
                } else {
                    {catalog}
                }
            }
        }
    }
}

/// 拍卖头部的开场/结束倒计时。秒级节拍只在本叶子组件内订阅：顶层 [`AuctionData`]
/// 不再持有每秒变化的时钟信号，目录子树不随秒针重渲染。
#[component]
fn AuctionHeaderCountdown(
    scheduled_ends_at: String,
    is_live_presentation: bool,
    status: String,
) -> Element {
    let tick = use_second_tick();
    let _ = tick();
    let countdown = countdown_display(&scheduled_ends_at, estimated_now());
    rsx! {
        p { class: "public-auction-round__end",
            if is_live_presentation {
                if status == "final" {
                    {t!("auction-header-final-ends-in")}
                } else {
                    {t!("auction-header-ends-in")}
                }
            } else {
                {t!("auction-header-opens-in")}
            }
            span {
                class: countdown_class(&countdown),
                aria_label: t!("auction-countdown"),
                "{countdown.label}"
            }
            Time { value: scheduled_ends_at.clone() }
        }
    }
}

/// lot 目录读取失败的本地化文案：先处理本页特有的历史变体码，未命中走共享基础层
/// （[`base_api_error_message`]），最后落到通用兜底。
fn lot_list_error_message(error: &str) -> String {
    match error {
        "request_cancelled" => return t!("error-request-cancelled"),
        "NOT_FOUND" => return t!("error-not-found"),
        "internal" | "INTERNAL" => return t!("error-internal"),
        _ => {}
    }
    base_api_error_message(error, |kind| match kind {
        BaseErrorKind::Network => t!("error-network"),
        BaseErrorKind::Timeout => t!("error-request-timeout"),
        BaseErrorKind::InvalidResponse => t!("error-invalid-response"),
        BaseErrorKind::NotFound => t!("error-not-found"),
        _ => t!("common-load-error"),
    })
    .unwrap_or_else(|| t!("common-load-error"))
}
