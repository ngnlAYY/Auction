//! Public 当前拍卖看板、世界选择和拍卖数据组件。

use super::*;

#[component]
pub(crate) fn AuctionIndex() -> Element {
    rsx! {
        AuctionBoard {}
    }
}

#[component]
pub(crate) fn AuctionPage() -> Element {
    redirect_public_alias("/")
}

/// 拍卖看板；首屏从同源 Public API 读取当前拍卖，不再渲染迁移阶段示例数据。
#[component]
pub(crate) fn AuctionBoard() -> Element {
    let selected_world = use_context::<Signal<String>>();
    let refresh_generation = use_context::<Signal<u64>>();
    let query = read_public_query();
    let requested_auction = query.auction_id.clone();
    let historical = requested_auction.is_some();
    let current = use_resource(move || {
        let _ = refresh_generation();
        let world = selected_world();
        let requested_auction = requested_auction.clone();
        async move {
            match requested_auction {
                Some(auction_id) => fetch_historical_auction(auction_id).await.map(Some),
                None => fetch_current_auction(world).await,
            }
        }
    });

    match &*current.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(None)) => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(auction))) => rsx! {
            AuctionData {
                auction: auction.clone(),
                historical,
                initial_category: query.category.clone().unwrap_or_else(|| "all".to_owned()),
                initial_page: query.page,
            }
        },
    }
}

/// 公开端世界选择器；世界是否可用由服务端目录决定，切换后重新读取权威当前拍卖。
#[component]
pub(crate) fn WorldPicker(mut selected_world: Signal<String>) -> Element {
    let worlds = use_resource(fetch_worlds);
    let options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let current = selected_world();
    let active = options
        .iter()
        .find(|world| **world == current)
        .cloned()
        .unwrap_or_else(|| options.first().cloned().unwrap_or_else(|| "main".to_owned()));
    let active_for_effect = active.clone();
    use_effect(move || {
        if selected_world() != active_for_effect {
            selected_world.set(active_for_effect.clone());
        }
    });
    let options_empty = options.is_empty();

    rsx! {
        label { class: "public-world-switcher",
            span { {t!("world-label")} }
            select {
                value: active.clone(),
                disabled: options_empty,
                onchange: move |event| {
                    let world = event.value();
                    selected_world.set(world.clone());
                    update_public_query(
                        |query| {
                            query.world = normalize_world_query(&world);
                            query.auction_id = None;
                        },
                        true,
                    );
                },
                for world in options {
                    option { value: world.clone(), {world_label(&world)} }
                }
            }
        }
    }
}

pub(crate) fn enabled_worlds(worlds: &Worlds) -> Vec<String> {
    worlds
        .worlds
        .iter()
        .filter(|(_, status)| status.enabled)
        .map(|(world, _): (&String, &WorldStatus)| world.clone())
        .collect()
}

pub(crate) fn world_label(world: &str) -> String {
    match world {
        "main" => t!("auction-world-main"),
        "isekai" => t!("auction-world-isekai"),
        _ => world.to_owned(),
    }
}

pub(crate) fn live_connection_class(state: LiveConnectionState) -> &'static str {
    match state {
        LiveConnectionState::Checking => "public-connection-badge--checking",
        LiveConnectionState::Connecting => "public-connection-badge--connecting",
        LiveConnectionState::Connected => "public-connection-badge--connected",
        LiveConnectionState::Anonymous => "public-connection-badge--anonymous",
        LiveConnectionState::Stopped => "public-connection-badge--stopped",
        LiveConnectionState::Error => "public-connection-badge--error",
    }
}

pub(crate) fn should_refresh_auction_board(event: &RealtimeEvent) -> bool {
    matches!(
        event.kind.as_str(),
        "auction.created"
            | "auction.preview_updated"
            | "preview_catalog_changed"
            | "auction.opened"
            | "auction.final_phase_entered"
            | "auction.cancelled"
            | "auction.settling"
            | "auction.settled"
            | "lot.bid_accepted"
    )
}
#[component]
pub(crate) fn AuctionData(
    auction: CurrentAuction,
    historical: bool,
    initial_category: String,
    initial_page: i64,
) -> Element {
    let mut clock = use_signal(estimated_now);
    let active_category = use_signal(|| initial_category.clone());
    let page_number = use_signal(|| initial_page.max(1));
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let should_refresh_at_open = !historical && auction.presentation_mode != "live";
    let opening_at =
        DateTime::parse_from_rfc3339(&auction.opens_at).ok().map(|value| value.with_timezone(&Utc));
    let opening_key = format!("{}:{}", auction.id, auction.opens_at);
    let mut opening_refresh_key = use_signal(|| None::<String>);
    let mut opening_refreshed = use_signal(|| false);
    let _clock_task = use_future({
        let opening_key = opening_key.clone();
        move || {
            let opening_key = opening_key.clone();
            async move {
                loop {
                    TimeoutFuture::new(1_000).await;
                    let now = estimated_now();
                    clock.set(now);
                    if opening_refresh_key().as_deref() != Some(opening_key.as_str()) {
                        opening_refresh_key.set(Some(opening_key.clone()));
                        opening_refreshed.set(false);
                    }
                    if should_refresh_at_open
                        && !opening_refreshed()
                        && realtime_state() == LiveConnectionState::Connected
                        && opening_at.is_some_and(|opens_at| now >= opens_at)
                    {
                        opening_refreshed.set(true);
                        refresh_generation.set(refresh_generation().saturating_add(1));
                    }
                }
            }
        }
    });
    let auction_id = auction.id.clone();
    let session = use_resource(fetch_public_session);
    let viewer_name = match &*session.read_unchecked() {
        Some(Ok(Some(session))) => Some(session.username.clone()),
        _ => None,
    };
    let authenticated = matches!(&*session.read_unchecked(), Some(Ok(Some(_))));
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
    let _auction_realtime = use_future({
        let realtime_auction_id = auction_id.clone();
        move || {
            let realtime_auction_id = realtime_auction_id.clone();
            async move {
                if historical {
                    return;
                }
                let mut after = 0_i64;
                loop {
                    let cursor = Rc::new(Cell::new(after));
                    let callback_cursor = Rc::clone(&cursor);
                    let mut callback_refresh = refresh_generation;
                    let result = subscribe_auction_realtime(
                        after,
                        realtime_auction_id.clone(),
                        move |page| {
                            let refetch_required = page.refetch_required;
                            callback_cursor.set(if refetch_required { 0 } else { page.next_after });
                            if refetch_required
                                || page.events.iter().any(should_refresh_auction_board)
                            {
                                callback_refresh.set(callback_refresh().saturating_add(1));
                            }
                        },
                    )
                    .await;
                    after = cursor.get();
                    let _ = result;
                    TimeoutFuture::new(5_000).await;
                }
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
    let aligned = match &*lots.read_unchecked() {
        Some(Ok(page)) => {
            page.auction_id == auction.id
                && page.world == auction.world
                && page.number == auction.number
                && page.auction_status == auction.status
                && page.presentation_mode == auction.presentation_mode
                && page.bidding_open == auction.bidding_open
                && page.preview_revision == auction.preview_revision
        }
        _ => true,
    };
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
    let now = clock();
    let is_live_presentation = auction.presentation_mode == "live";
    let auction_end = if is_live_presentation {
        auction.scheduled_ends_at.clone()
    } else {
        Some(auction.opens_at.clone())
    };
    let auction_countdown = auction_end.as_deref().map(|ends_at| countdown_display(ends_at, now));
    let catalog = match &*lots.read_unchecked() {
        None => {
            rsx! { p { class: "public-auction-page__status", role: "status", {t!("data-loading")} } }
        }
        Some(Err(_)) => {
            rsx! {
                RetryPlaceholder {
                    title: t!("auction-title"),
                    detail: t!("data-error"),
                    refresh_generation: lots_refresh_generation,
                }
            }
        }
        Some(Ok(_)) if !aligned => rsx! {
            RetryPlaceholder {
                title: t!("auction-title"),
                detail: t!("data-aligning"),
                refresh_generation: lots_refresh_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            CatalogTable {
                rows: page.rows(),
                now,
                projection_kind: page.projection_kind.clone(),
                category_counts: page.category_counts.clone().unwrap_or_default(),
                active_category,
                page_number,
                page_total: page.total.unwrap_or_else(|| page.rows().len() as i64),
                page_size: page.page_size.filter(|size| *size > 0).unwrap_or(20),
                can_bid: is_live_presentation && auction.bidding_open,
                allow_slow: auction.status == "live",
                authenticated,
                viewer_name: viewer_name.clone(),
            }
        },
    };

    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-title",
            if historical {
                HistoricalAuctionNavigation { auction: auction.clone() }
            }
            header { class: "public-auction-round",
                div { class: "public-auction-round__title",
                    h1 { id: "auction-title", {t!("auction-title")}, " #{auction.number}" }
                    span { class: "public-auction-round__world", "{auction_world_label(&auction.world)}" }
                    span {
                        class: "public-auction-round__status public-auction-round__status--{auction_status_class(&auction.status)}",
                        "{status_label}"
                    }
                }
                if let (Some(scheduled_ends_at), Some(countdown)) =
                    (auction_end.clone(), auction_countdown)
                {
                    p { class: "public-auction-round__end",
                        if is_live_presentation { {t!("auction-ends-at")} } else { {t!("auction-opens-at")} }
                        time { datetime: scheduled_ends_at.clone(), "{format_auction_time(&scheduled_ends_at)}" }
                        span {
                            class: countdown_class(&countdown),
                            aria_label: t!("auction-countdown"),
                            "{countdown.label}"
                        }
                    }
                }
                {preview_history_switch(&history_switch.read_unchecked(), &auction, historical)}
            }

            {catalog}
            if !historical && is_live_presentation {
                {forum::panel(auction.id.clone())}
            }
        }
    }
}

pub(crate) fn preview_history_switch(
    state: &Option<PreviewHistoryLoad>,
    auction: &CurrentAuction,
    historical: bool,
) -> Element {
    if !historical && auction.presentation_mode == "live" {
        return rsx! {};
    }

    let Some(state) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-loading")} }
                }
            }
        };
    };
    let Ok(Some((current, rounds))) = state else {
        return rsx! {
            nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
                span { class: "public-auction-view-switch__options",
                    span { class: "public-auction-view-switch__status", {t!("data-error")} }
                }
            }
        };
    };
    let Some(current) = current else {
        return rsx! {};
    };
    if current.presentation_mode == "live" {
        return rsx! {};
    }

    let latest_settled = rounds
        .iter()
        .filter(|round| round.world_id == current.world)
        .max_by_key(|round| round.sequence);
    if historical && latest_settled.is_none_or(|round| round.id != auction.id) {
        return rsx! {};
    }

    let mut current_query = read_public_query();
    current_query.auction_id = None;
    current_query.world = normalize_world_query(&current.world);
    let current_href = public_href("/auction", &current_query);
    let previous = rounds
        .iter()
        .filter(|round| round.world_id == current.world && round.sequence < current.number)
        .max_by_key(|round| round.sequence);
    let previous_href = previous.map(|round| {
        let mut query = read_public_query();
        query.auction_id = Some(round.id.clone());
        query.world = normalize_world_query(&round.world_id);
        public_href("/auction", &query)
    });

    rsx! {
        nav { class: "public-auction-view-switch", aria_label: t!("auction-history-view-label"),
            span { class: "public-auction-view-switch__options",
                if historical {
                    a { href: current_href, {t!("auction-history-current")} }
                } else if let Some(previous_href) = previous_href {
                    a { href: previous_href, {t!("auction-history-previous-view")} }
                } else {
                    span { {t!("auction-history-no-previous")} }
                }
            }
        }
    }
}

/// 历史拍卖页的返回和上一轮导航；当前轮次不显示历史导航，避免污染首屏竞价操作。
#[component]
pub(crate) fn HistoricalAuctionNavigation(auction: CurrentAuction) -> Element {
    let history = use_resource(fetch_auction_list);
    let previous = match &*history.read_unchecked() {
        Some(Ok(rows)) => rows
            .iter()
            .filter(|row| row.world_id == auction.world && row.sequence < auction.number)
            .max_by_key(|row| row.sequence)
            .cloned(),
        _ => None,
    };

    rsx! {
        nav {
            class: "public-auction-history-nav",
            aria_label: t!("auction-history-navigation"),
            a { href: "/auctions", {t!("auction-history-back")} }
            if let Some(previous) = previous {
                a {
                    href: historical_auction_href(&previous.id, Some(&previous.world_id)),
                    {t!("auction-history-previous", number: previous.sequence)}
                }
            }
        }
    }
}
