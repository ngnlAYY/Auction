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
    // 生命周期、可见页定时和人工刷新先重读目录；记录请求随后只消费本轮确认的公开场次。
    let auctions = use_resource(move || {
        let generation = refresh_generation();
        let world = world_filter();
        async move {
            let result = fetch_bid_log_auctions(world.clone()).await;
            ((world, generation), result)
        }
    });

    // 保留仍可公开读取的历史选择；仅在所选场次不属于本世界目录时纠偏。
    use_effect(move || {
        let Some(((world, generation), Ok(auction_list))) = &*auctions.read_unchecked() else {
            return;
        };
        // 后台刷新保留旧目录用于展示，但它不能纠偏新世界或新生命周期代际的选择。
        if world != &world_filter() || *generation != refresh_generation() {
            return;
        }
        let selected = selected_auction_id();
        let next_auction_id = auction_list
            .iter()
            .find(|auction| auction.id == selected)
            .or_else(|| auction_list.first())
            .map(|auction| auction.id.clone());
        let next_auction_id = next_auction_id.unwrap_or_default();
        if selected != next_auction_id {
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

    let bid_log = use_resource(move || {
        let auction_id = selected_auction_id();
        let page = page_number();
        let page_size = page_size();
        let world = world_filter();
        let lot_id = lot_filter().trim().to_owned();
        let lot_id = (!lot_id.is_empty()).then_some(lot_id);
        let requested = (auction_id.clone(), world.clone(), lot_id.clone(), page, page_size);
        // 不直接订阅全局刷新：先确认本轮目录，避免按过期的场次可见性读取记录。
        // 目录未就绪、失败或已移除该场时不发 GET；查询切换也不能绕过这道门禁。
        let readable = auctions.read_unchecked().as_ref().is_some_and(
            |((loaded_world, generation), result)| {
                loaded_world == &world
                    && *generation == *refresh_generation.peek()
                    && result
                        .as_ref()
                        .is_ok_and(|rows| rows.iter().any(|auction| auction.id == auction_id))
            },
        );
        async move {
            let result = if readable {
                fetch_bid_log_page(auction_id, page, world, lot_id, page_size).await.map(Some)
            } else {
                Ok(None)
            };
            (requested, result)
        }
    });
    let auction_snapshot = auctions.read_unchecked();
    let current_auctions = auction_snapshot
        .as_ref()
        .filter(|((world, _), _)| world == &world_filter())
        .map(|(_, result)| result);
    let auction_options = match current_auctions {
        Some(Ok(auction_list)) => auction_list.as_slice(),
        _ => &[],
    };
    let selected = selected_auction_id();
    let selected_auction = auction_options.iter().find(|auction| auction.id == selected);
    let directory_current = auction_snapshot.as_ref().is_some_and(|((world, generation), _)| {
        world == &world_filter() && *generation == refresh_generation()
    });
    use_bid_refresh(bid_log, directory_current && selected_auction.is_some());
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
    // 同查询后台刷新保留完成快照；目录确认场次已不可见后，迟到记录也不能恢复旧行和分页。
    let current_result = bid_log_state
        .as_ref()
        .filter(|((auction_id, world, lot_id, page, size), _)| {
            selected_auction.is_some()
                && auction_id == &selected_auction_id()
                && world == &world_filter()
                && lot_id.as_deref().unwrap_or_default() == current_lot_filter.trim()
                && *page == current_page
                && *size == page_size()
        })
        .map(|(_, result)| result);
    let body = match (current_auctions, current_result) {
        (Some(Err(_)), _) | (Some(Ok(_)), Some(Err(_))) => rsx! {
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
        (Some(Ok(auction_list)), _) if auction_list.is_empty() => rsx! {
            div { class: "public-auction-empty", role: "status",
                p { class: "public-auction-empty__title", {t!("data-empty")} }
            }
        },
        (Some(Ok(_)), Some(Ok(Some(page))))
            if public_page_matches(page.page, page.page_size, current_page, page_size()) =>
        {
            rsx! {
                BidLogTable {
                    page: page.clone(),
                    current_page,
                    world: world_filter(),
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
            }
        }
        _ => rsx! {
            p { class: "public-auction-bid-log-empty", role: "status", {t!("data-loading")} }
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
                        name: "world",
                        autocomplete: "off",
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
                        for world in &world_options {
                            option { value: world.clone(), {world_label(world)} }
                        }
                    }
                }
                label { r#for: "public-auction-bid-log-round",
                    span { {t!("bidlog-round")} }
                    select {
                        id: "public-auction-bid-log-round",
                        name: "auction",
                        autocomplete: "off",
                        value: selected,
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
                        for auction in auction_options {
                            option {
                                value: auction.id.clone(),
                                {t!("bidlog-round-option", number: auction.sequence,
                                    status: bid_log_status_label(&auction.status))}
                            }
                        }
                    }
                }
            }
            div { class: "public-auction-bid-log-heading",
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
                        name: "lot",
                        autocomplete: "off",
                        r#type: "search",
                        enterkeyhint: "search",
                        autocapitalize: "none",
                        spellcheck: "false",
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
            }
            {body}
        }
    }
}

#[component]
pub(crate) fn BidLogTable(
    page: BidLogPage,
    current_page: i64,
    world: String,
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
                    class: "public-auction-bid-log-table",
                    caption { class: "public-visually-hidden", {t!("page-bidlog-title")} }
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
                            BidLogRow { key: "{bid.id}", bid, world: world.clone() }
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
pub(crate) fn BidLogRow(bid: BidLogEntry, world: String) -> Element {
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
                Link { to: crate::app::surface::lot_detail_world_href(&lot_id, &world), "{lot_code}" }
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
