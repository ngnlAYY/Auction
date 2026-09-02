use super::*;

pub(crate) fn bid_log() -> Element {
    let mut selected_world = use_context::<Signal<String>>();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let query = read_public_query();
    let mut selected_auction_id = use_signal(|| query.auction_id.clone().unwrap_or_default());
    let mut page_number = use_signal(|| query.page);
    let mut lot_filter = use_signal(|| query.bid_log_lot.clone().unwrap_or_default());
    let canonical_query = query.clone();
    use_effect(move || {
        if canonical_query.bid_log_page_alias {
            // 迁移期间曾写出 `bidlog_page`；读取后立即无历史记录地收敛回旧版 `page`。
            set_public_query(&canonical_query, true);
        }
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec![selected_world()],
    };
    let auctions = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            fetch_bid_log_auctions(selected_world())
        }
    });

    // 世界切换或分享链接指向另一世界时，自动落到该世界最新一场可公开读取的拍卖。
    use_effect({
        let mut selected_auction_id = selected_auction_id;
        let mut page_number = page_number;
        move || {
            let Some(Ok(auction_list)) = &*auctions.read_unchecked() else {
                return;
            };
            let next_auction_id = auction_list
                .iter()
                .find(|auction| auction.id == selected_auction_id())
                .or_else(|| auction_list.first())
                .map(|auction| auction.id.clone());
            let next_auction_id = next_auction_id.unwrap_or_default();
            if selected_auction_id() != next_auction_id {
                selected_auction_id.set(next_auction_id.clone());
                page_number.set(1);
                update_public_query(
                    |query| {
                        query.auction_id =
                            (!next_auction_id.is_empty()).then_some(next_auction_id.clone());
                        query.page = 1;
                    },
                    true,
                );
            }
        }
    });

    let bid_log = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = page_number();
            let lot_id = lot_filter().trim().to_owned();
            let lot_id = Uuid::parse_str(&lot_id).ok().map(|id| id.to_string());
            async move {
                if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_bid_log_page(auction_id, page, selected_world(), lot_id).await.map(Some)
                }
            }
        }
    });
    let auction_options = match &*auctions.read_unchecked() {
        Some(Ok(auction_list)) => auction_list.clone(),
        _ => Vec::new(),
    };
    let selected_auction =
        auction_options.iter().find(|auction| auction.id == selected_auction_id()).cloned();
    let current_page = page_number();
    let current_lot_filter = lot_filter();
    let body = match &*bid_log.read_unchecked() {
        None => rsx! {
            p { class: "public-auction-bid-log-empty", role: "status", {t!("data-loading")} }
        },
        Some(Err(_)) => rsx! {
            div { class: "public-auction-empty", role: "alert",
                p { class: "public-auction-empty__title", {t!("data-error")} }
                button {
                    class: "public-auction-empty__retry",
                    r#type: "button",
                    onclick: move |_| refresh_generation.set(refresh_generation().saturating_add(1)),
                    {t!("data-retry")}
                }
            }
        },
        Some(Ok(None)) => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("bidlog-no-rounds")} }
            }
        },
        Some(Ok(Some(page))) if page.bids.is_empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("bidlog-empty")} }
            }
        },
        Some(Ok(Some(page))) => rsx! {
            BidLogTable {
                page: page.clone(),
                current_page,
                lot_filter: current_lot_filter.clone(),
                on_lot_filter_change: move |value: String| {
                    lot_filter.set(value.clone());
                    page_number.set(1);
                    update_public_query(
                        |query| {
                            query.bid_log_lot = (!value.trim().is_empty()).then_some(value);
                            query.page = 1;
                        },
                        false,
                    );
                },
                on_page_change: move |next_page: i64| {
                    page_number.set(next_page);
                    update_public_query(|query| query.page = next_page, false);
                },
            }
        },
    };

    rsx! {
        section { class: "public-auction-bid-log-page", aria_labelledby: "bid-log-title",
            h1 { id: "bid-log-title", {t!("page-bidlog-title")} }
            div { class: "public-auction-bid-log-controls",
                label { r#for: "public-auction-bid-log-world",
                    span { {t!("world-label")} }
                    select {
                        id: "public-auction-bid-log-world",
                        value: selected_world(),
                        disabled: world_options.is_empty(),
                        onchange: move |event| {
                            let world = event.value();
                            selected_world.set(world.clone());
                            selected_auction_id.set(String::new());
                            lot_filter.set(String::new());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    // 旧版 Main 使用缺省 URL，非 Main 世界才写入 `world`。
                                    query.world = (world != "main").then_some(world.clone());
                                    query.auction_id = None;
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        for world in world_options.clone() {
                            option { value: world.clone(), {world_label(&world)} }
                        }
                    }
                }
                label { r#for: "public-auction-bid-log-round",
                    span { {t!("bidlog-round")} }
                    select {
                        id: "public-auction-bid-log-round",
                        value: selected_auction_id(),
                        disabled: auction_options.is_empty(),
                        onchange: move |event| {
                            let auction_id = event.value();
                            selected_auction_id.set(auction_id.clone());
                            lot_filter.set(String::new());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.auction_id = Some(auction_id.clone());
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        for auction in auction_options.clone() {
                            option {
                                value: auction.id.clone(),
                                {t!("bidlog-round-option", number: auction.sequence,
                                    status: bid_log_status_label(&auction.status))}
                            }
                        }
                    }
                }
            }
            if let Some(auction) = selected_auction {
                div { class: "public-auction-bid-log-round-header",
                    strong { class: "public-auction-bid-log-round-number",
                        {t!("bidlog-round-number", number: auction.sequence)}
                    }
                    span { class: "public-auction-bid-log-round-world",
                        {t!("bidlog-world", world: super::world_label(&auction.world_id))}
                    }
                    span { class: bid_log_status_class(&auction.status),
                        {bid_log_status_label(&auction.status)}
                    }
                }
            }
            {body}
        }
    }
}

#[component]
pub(crate) fn BidLogTable(
    page: BidLogPage,
    current_page: i64,
    lot_filter: String,
    on_lot_filter_change: EventHandler<String>,
    on_page_change: EventHandler<i64>,
) -> Element {
    let has_more = page.has_more.unwrap_or(false);
    let needle = lot_filter.trim().to_ascii_lowercase();
    let server_filtered = Uuid::parse_str(lot_filter.trim()).is_ok();
    let bids = if needle.is_empty() || server_filtered {
        page.bids.clone()
    } else {
        page.bids
            .iter()
            .filter(|bid| {
                lot_code_for(&bid.equipment_category, bid.lot_number)
                    .to_ascii_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .collect()
    };
    rsx! {
        div {
            div { class: "public-auction-bid-log-toolbar",
                label { r#for: "public-auction-bid-log-filter", {t!("bidlog-filter-label")} }
                input {
                    id: "public-auction-bid-log-filter",
                    value: lot_filter.clone(),
                    placeholder: t!("bidlog-filter-placeholder"),
                    oninput: move |event| on_lot_filter_change.call(event.value()),
                }
                if !lot_filter.is_empty() {
                    button {
                        r#type: "button",
                        onclick: move |_| on_lot_filter_change.call(String::new()),
                        {t!("bidlog-filter-clear")}
                    }
                }
            }
            if bids.is_empty() {
                p { class: "public-auction-bid-log-empty", {t!("bidlog-filter-empty")} }
            } else {
            div { class: "public-auction-bid-log-table-wrap",
                table {
                    class: "public-auction-bid-log-table",
                    thead {
                        tr {
                            th { {t!("bidlog-time")} }
                            th { {t!("bidlog-lot")} }
                            th { {t!("bidlog-bidder")} }
                            th { class: "public-auction-bid-log-number", {t!("bidlog-amount")} }
                        }
                    }
                    tbody {
                        for bid in bids {
                            BidLogRow { bid }
                        }
                    }
                }
            }
            }
            PublicPager {
                page: current_page,
                page_size: page.page_size,
                total: page.total,
                has_more,
                label: t!("page-bidlog-title"),
                on_page_change,
            }
        }
    }
}

#[component]
pub(crate) fn BidLogRow(bid: BidLogEntry) -> Element {
    let lot_code = lot_code_for(&bid.equipment_category, bid.lot_number);
    let lot_id = bid.lot_id.clone();
    let validity = if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") };
    rsx! {
        tr {
            class: if bid.valid { "public-auction-bid-log-leading" } else { "" },
            td { "data-label": t!("bidlog-time"),
                time { datetime: bid.created_at.clone(), "{format_auction_time(&bid.created_at)}" }
            }
            td { class: "public-auction-bid-log-code", "data-label": t!("bidlog-lot"),
                Link { to: Route::LotDetail { lot_code: lot_id }, "{lot_code}" }
            }
            td { class: "public-auction-bid-log-participant", "data-label": t!("bidlog-bidder"),
                BidHistoryParticipant {
                    name: bid.bidder_name,
                    source: bid.source,
                    source_post_number: bid.source_post_number,
                    source_post_url: bid.source_post_url,
                }
                span { class: "public-visually-hidden", "{validity}" }
            }
            td {
                class: "public-auction-bid-log-amount public-auction-bid-log-number",
                "data-label": t!("bidlog-amount"),
                "{super::format_amount(bid.amount)}"
            }
        }
    }
}

pub(crate) fn bid_log_status_label(status: &str) -> String {
    match status {
        "live" => t!("auction-history-status-live"),
        "final" => t!("auction-history-status-final"),
        "settled" => t!("auction-history-status-settled"),
        _ => t!("bidlog-status-unknown"),
    }
}

pub(crate) fn bid_log_status_class(status: &str) -> &'static str {
    match status {
        "live" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-live",
        "final" => "public-auction-bid-log-round-status public-auction-bid-log-round-status-final",
        _ => "public-auction-bid-log-round-status",
    }
}
