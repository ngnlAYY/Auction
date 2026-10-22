use super::*;
use crate::app::surface::use_bid_refresh;
use gloo_timers::future::TimeoutFuture;

/// lot 筛选写回地址栏的防抖窗口：输入立即刷新列表，历史条目在停顿后合并为一次 replace。
const LOT_FILTER_WRITE_DEBOUNCE_MILLIS: u32 = 300;

pub(crate) fn bid_log() -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    // 世界/拍卖/页码/lot 筛选与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let mut query_state_signal = query_state.state;
    let world_filter = use_memo(move || {
        query_state_signal
            .read()
            .world
            .clone()
            .filter(|world| matches!(world.as_str(), "main" | "isekai"))
            .unwrap_or_else(|| "main".to_owned())
    });
    let selected_auction_id =
        use_memo(move || query_state_signal.read().auction_id.clone().unwrap_or_default());
    let page_number = use_memo(move || query_state_signal.read().page);
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let lot_filter =
        use_memo(move || query_state_signal.read().bid_log_lot.clone().unwrap_or_default());
    let mut lot_write_generation = use_signal(|| 0_u64);
    // 迁移期间曾写出 `bidlog_page`；读取后立即无历史记录地收敛回旧版 `page`。
    use_effect(move || {
        if query_state_signal().bid_log_page_alias {
            let mut next = query_state_signal.peek().clone();
            next.bid_log_page_alias = false;
            write_public_query(&next, QueryWriteMode::Replace);
            query_state_signal.set(next);
        }
    });
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec![world_filter()],
    };
    let auctions = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let world = world_filter();
            async move {
                let result = fetch_bid_log_auctions(world.clone()).await;
                (world, result)
            }
        }
    });

    // 世界切换或分享链接指向另一世界时，自动落到该世界最新一场可公开读取的拍卖。
    use_effect(move || {
        let Some((world, Ok(auction_list))) = &*auctions.read_unchecked() else {
            return;
        };
        // 世界切换期间 Resource 保留旧目录，不能据它纠偏新世界的场次选择。
        if world != &world_filter() {
            return;
        }
        let next_auction_id = auction_list
            .iter()
            .find(|auction| auction.id == selected_auction_id())
            .or_else(|| auction_list.first())
            .map(|auction| auction.id.clone());
        let next_auction_id = next_auction_id.unwrap_or_default();
        if selected_auction_id() != next_auction_id {
            update_public_query_state(
                query_state_signal,
                |query| {
                    query.auction_id =
                        (!next_auction_id.is_empty()).then_some(next_auction_id.clone());
                    query.page = 1;
                },
                QueryWriteMode::Replace,
            );
        }
    });

    let bid_log = use_resource({
        let refresh_generation = refresh_generation;
        move || {
            let _ = refresh_generation();
            let auction_id = selected_auction_id();
            let page = page_number();
            let page_size = page_size();
            let world = world_filter();
            let lot_id = lot_filter().trim().to_owned();
            let lot_id = (!lot_id.is_empty()).then_some(lot_id);
            let requested = (auction_id.clone(), world.clone(), lot_id.clone(), page, page_size);
            async move {
                let result = if auction_id.is_empty() {
                    Ok(None)
                } else {
                    fetch_bid_log_page(auction_id, page, world, lot_id, page_size).await.map(Some)
                };
                (requested, result)
            }
        }
    });
    use_bid_refresh(bid_log, true);
    let auction_options = match &*auctions.read_unchecked() {
        Some((world, Ok(auction_list))) if world == &world_filter() => auction_list.clone(),
        _ => Vec::new(),
    };
    let selected_auction =
        auction_options.iter().find(|auction| auction.id == selected_auction_id()).cloned();
    let current_page = page_number();
    let current_lot_filter = lot_filter();
    let on_lot_filter_change = use_callback(move |value: String| {
        // 输入立即更新状态，地址栏防抖写回；工具条不随请求结果卸载，以保留焦点。
        let mut next = query_state_signal.peek().clone();
        next.bid_log_lot = (!value.trim().is_empty()).then_some(value);
        next.page = 1;
        query_state_signal.set(next);
        let generation = lot_write_generation.peek().saturating_add(1);
        lot_write_generation.set(generation);
        spawn(async move {
            TimeoutFuture::new(LOT_FILTER_WRITE_DEBOUNCE_MILLIS).await;
            // 迟到任务不能改写已经离开的页面或更新的输入。
            if *lot_write_generation.peek() == generation && current_public_pathname() == "/bidlog"
            {
                let next = query_state_signal.peek().clone();
                write_public_query(&next, QueryWriteMode::Replace);
            }
        });
    });
    let bid_log_state = bid_log.read_unchecked();
    // 同一查询的后台刷新保留完成快照；换世界、场次或 lot 后隐藏旧行和旧分页总数。
    let current_result = bid_log_state
        .as_ref()
        .filter(|((auction_id, world, lot_id, page, size), _)| {
            auction_id == &selected_auction_id()
                && world == &world_filter()
                && lot_id.as_deref().unwrap_or_default() == current_lot_filter.trim()
                && *page == current_page
                && *size == page_size()
        })
        .map(|(_, result)| result);
    let body = match current_result {
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
        Some(Ok(Some(page)))
            if !public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                p { role: "status", {t!("data-loading")} }
            }
        }
        Some(Ok(Some(page))) => rsx! {
            BidLogTable {
                page: page.clone(),
                current_page,
                lot_filter: current_lot_filter.clone(),
                on_page_change: move |next_page: i64| {
                    // lot 输入可能仍在等待防抖写回；从当前状态提交，不能用旧 URL 丢掉筛选。
                    let mut next = query_state_signal.peek().clone();
                    next.page = next_page;
                    write_public_query(&next, QueryWriteMode::Push);
                    query_state_signal.set(next);
                },
                on_page_clamp: move |next_page: i64| {
                    // 纠偏同样保留尚未防抖写回的 lot 筛选，不建立可返回的无效页条目。
                    let mut next = query_state_signal.peek().clone();
                    next.page = next_page;
                    write_public_query(&next, QueryWriteMode::Replace);
                    query_state_signal.set(next);
                },
                on_page_size_change: move |size| {
                    let mut next = query_state_signal.peek().clone();
                    next.page_size = super::super::normalize_public_page_size(size);
                    next.page = 1;
                    write_public_query(&next, QueryWriteMode::Push);
                    query_state_signal.set(next);
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
                        value: world_filter(),
                        disabled: world_options.is_empty(),
                        onchange: move |event| {
                            let world = event.value();
                            update_public_query_state(
                                query_state_signal,
                                |query| {
                                    // 旧版 Main 使用缺省 URL，非 Main 世界才写入 `world`。
                                    query.world = (world != "main").then_some(world.clone());
                                    query.auction_id = None;
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                QueryWriteMode::Push,
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
                            update_public_query_state(
                                query_state_signal,
                                |query| {
                                    query.auction_id = Some(auction_id.clone());
                                    query.bid_log_lot = None;
                                    query.page = 1;
                                },
                                QueryWriteMode::Push,
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
            div { class: "public-auction-bid-log-toolbar",
                label { r#for: "public-auction-bid-log-filter", {t!("bidlog-filter-label")} }
                input {
                    id: "public-auction-bid-log-filter",
                    value: current_lot_filter.clone(),
                    placeholder: t!("bidlog-filter-placeholder"),
                    oninput: move |event| on_lot_filter_change.call(event.value()),
                }
                if !current_lot_filter.is_empty() {
                    button {
                        r#type: "button",
                        onclick: move |_| on_lot_filter_change.call(String::new()),
                        {t!("bidlog-filter-clear")}
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
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    let has_more = page.has_more.unwrap_or(false);
    let bids = page.bids.clone();
    rsx! {
        div {
            if bids.is_empty() {
                p { class: "public-auction-bid-log-empty",
                    if lot_filter.is_empty() {
                        {t!("bidlog-empty")}
                    } else {
                        {t!("bidlog-filter-empty")}
                    }
                }
            } else {
            div { class: "public-auction-bid-log-table-wrap",
                table {
                    class: "public-auction-bid-log-table ui-table--responsive",
                    auction_web_shared::TableColumns { layout: "public-bid-log", widths: vec![30, 12, 40, 18] }
                    thead {
                        tr {
                            th { scope: "col", {t!("bidlog-time")} }
                            th { scope: "col", {t!("bidlog-lot")} }
                            th { scope: "col", {t!("bidlog-bidder")} }
                            th { scope: "col", class: "public-auction-bid-log-number", {t!("bidlog-amount")} }
                        }
                    }
                    tbody {
                        for bid in bids {
                            // 以服务端 bid id 作为稳定 key，避免翻页/筛选刷新后行状态错位。
                            BidLogRow { key: "{bid.id}", bid }
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
                on_page_clamp,
                on_page_size_change,
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
                Time { value: bid.created_at.clone() }
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
