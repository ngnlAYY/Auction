//! Public 拍卖数据加载、倒计时和目录展示编排。

use super::*;

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
                        && realtime_state() == LiveConnectionState::Live
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
    let session = use_context::<PublicSessionResource>();
    let session_snapshot = session.read().clone();
    let viewer_name = match session_snapshot.as_ref() {
        Some(Ok(Some(session))) if session.authenticated => Some(session.username.clone()),
        _ => None,
    };
    let session_ready = matches!(session_snapshot.as_ref(), Some(Ok(_)));
    let authenticated = matches!(
        session_snapshot.as_ref(),
        Some(Ok(Some(session))) if session.authenticated
    );
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
    let connection_state = realtime_state();
    let connection_state_name = live_connection_state_name(connection_state);
    let connection_message = super::super::realtime::live_connection_message(connection_state);
    let auction_end = if is_live_presentation {
        auction.scheduled_ends_at.clone()
    } else {
        Some(auction.opens_at.clone())
    };
    let auction_countdown = auction_end.as_deref().map(|ends_at| countdown_display(ends_at, now));
    let catalog = match &*lots.read_unchecked() {
        None => {
            rsx! { p { class: "public-page-status", role: "status", {t!("common-loading")} } }
        }
        Some(Err(error)) => {
            rsx! {
                CatalogLoadFailure {
                    detail: lot_list_error_message(error),
                    refresh_generation: lots_refresh_generation,
                }
            }
        }
        Some(Ok(_)) if !aligned => rsx! {
            p { class: "public-page-status", role: "status", {t!("common-loading")} }
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
                session_ready,
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
            if !historical && is_live_presentation {
                {forum::panel(auction.id.clone())}
            }
            div { class: "public-auction-live",
                if !historical {
                    p {
                        aria_live: "polite",
                        class: "public-visually-hidden",
                        "data-state": connection_state_name,
                        {connection_message}
                    }
                }
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
                    if let (Some(scheduled_ends_at), Some(countdown)) =
                        (auction_end.clone(), auction_countdown)
                    {
                        p { class: "public-auction-round__end",
                            if is_live_presentation {
                                {t!("auction-header-ends-in")}
                            } else {
                                {t!("auction-header-opens-in")}
                            }
                            span {
                                class: countdown_class(&countdown),
                                aria_label: t!("auction-countdown"),
                                "{countdown.label}"
                            }
                            time { datetime: scheduled_ends_at.clone(), "{format_auction_time(&scheduled_ends_at)}" }
                        }
                    }
                    {preview_history_switch(&history_switch.read_unchecked(), &auction, historical)}
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

#[component]
fn CatalogLoadFailure(detail: String, mut refresh_generation: Signal<u64>) -> Element {
    rsx! {
        div { class: "ui-empty-state", role: "alert",
            p { class: "ui-empty-state__title", {t!("catalog-empty-all")} }
            span { class: "ui-empty-state__detail", "{detail}" }
            button {
                class: "ui-empty-state__retry",
                r#type: "button",
                onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                {t!("data-retry")}
            }
        }
    }
}

fn lot_list_error_message(error: &str) -> String {
    match error {
        "invalid_response" => t!("error-invalid-response"),
        "network_error" => t!("error-network"),
        "request_timeout" => t!("error-request-timeout"),
        "request_cancelled" => t!("error-request-cancelled"),
        "not_found" | "NOT_FOUND" => t!("error-not-found"),
        "internal" | "INTERNAL" => t!("error-internal"),
        _ => t!("common-load-error"),
    }
}
