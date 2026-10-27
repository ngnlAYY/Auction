//! Public 历史拍卖列表。

use super::super::*;
use crate::api::{HistoricalAuctionWithForum, fetch_auction_history_page};

#[component]
pub(crate) fn AuctionList() -> Element {
    // 世界/状态/页码与地址栏双向同步：初值取自 URL，浏览器前进/后退由 hook 恢复。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let world_filter = use_memo(move || {
        query_state_signal.read().world.clone().unwrap_or_else(|| "all".to_owned())
    });
    let status_filter = use_memo(move || {
        let status = query_state_signal.read().status.clone();
        status
            .filter(|status| matches!(status.as_str(), "live" | "final" | "settled"))
            .unwrap_or_else(|| "settled".to_owned())
    });
    let page_number = use_memo(move || query_state_signal.read().page.max(1));
    let page_size = use_memo(move || query_state_signal.read().page_size);
    let mut refresh_generation = use_signal(|| 0_u64);
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let auctions = use_resource(move || {
        let _ = refresh_generation();
        let world = world_filter();
        let status = status_filter();
        let page = page_number();
        let size = page_size();
        let requested = (world.clone(), status.clone(), page, size);
        let world = (world != "all").then_some(world);
        async move {
            let result = fetch_auction_history_page(world, status, page, size).await;
            (requested, result)
        }
    });
    let auction_state = auctions.read_unchecked();
    // 查询变化时旧快照不能参与列表展示或页码纠偏，同查询后台刷新仍保留旧值。
    let current_result = auction_state
        .as_ref()
        .filter(|((world, status, page, size), _)| {
            world == &world_filter()
                && status == &status_filter()
                && *page == page_number()
                && *size == page_size()
        })
        .map(|(_, result)| result);
    match current_result {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("page-auctions-title"),
                detail: t!("data-loading"),
            }
        },
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        },
        Some(Ok(page))
            if !public_page_matches(page.page, page.page_size, page_number(), page_size()) =>
        {
            rsx! {
                p { role: "status", {t!("data-loading")} }
            }
        }
        Some(Ok(page)) => rsx! {
            AuctionHistoryPage {
                page: page.clone(),
                world_filter: world_filter(),
                page_number: page_number(),
                world_options,
                on_page_size_change: move |size| update_public_page_size(query_state_signal, size),
                on_world_change: move |world: String| {
                    update_public_query_state(
                        query_state_signal,
                        |query| {
                            query.world = (world != "all").then_some(world.clone());
                            query.auction_id = None;
                            query.category = None;
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
                on_page_clamp: move |page| {
                    update_public_query_state(
                        query_state_signal,
                        |query| query.page = page,
                        QueryWriteMode::Replace,
                    );
                },
            }
        },
    }
}

#[component]
pub(crate) fn AuctionHistoryPage(
    page: AuctionListPage<HistoricalAuctionWithForum>,
    world_filter: String,
    page_number: i64,
    world_options: Vec<String>,
    on_world_change: EventHandler<String>,
    on_page_change: EventHandler<i64>,
    on_page_clamp: EventHandler<i64>,
    on_page_size_change: EventHandler<i64>,
) -> Element {
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-auction-rounds", aria_labelledby: "auction-history-title",
            h1 { id: "auction-history-title", {t!("page-auctions-title")} }
            p { class: "public-auction-history-intro", {t!("page-auctions-note")} }
            div { class: "public-auction-rounds__controls",
                label {
                    {t!("auction-history-world")}
                    select {
                        value: world_filter,
                        onchange: move |event| on_world_change.call(event.value()),
                        option { value: "all", {t!("auction-history-world-all")} }
                        for world in world_options {
                            option { value: world.clone(), {world_label(&world)} }
                        }
                    }
                }
            }
            if page.auctions.is_empty() {
                div { class: "public-auction-empty", role: "status",
                    p { class: "public-auction-empty__title", {t!("auction-history-empty-title")} }
                    p { class: "public-auction-empty__hint", {t!("auction-history-empty-hint")} }
                }
            } else {
                AuctionHistoryTable { rows: page.auctions }
            }
            PublicPager {
                page: page_number,
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("page-auctions-title"),
                on_page_change,
                on_page_clamp,
                on_page_size_change,
            }
        }
    }
}

#[component]
pub(crate) fn AuctionHistoryTable(rows: Vec<HistoricalAuctionWithForum>) -> Element {
    rsx! {
        div { class: "public-auction-rounds__table",
            table {
                caption { class: "public-visually-hidden", {t!("page-auctions-title")} }
                auction_web_shared::TableColumns { layout: "public-auction-history", widths: vec![12, 8, 22, 7, 7, 7, 21, 16] }
                thead {
                    tr {
                        th { scope: "col", {t!("auction-history-number")} }
                        th { scope: "col", {t!("auction-history-world")} }
                        th { scope: "col", {t!("auction-history-ended")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-lots")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-sold")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-unsold")} }
                        th { scope: "col", class: "public-auction-rounds__number", {t!("auction-history-total")} }
                        th { scope: "col", {t!("auction-history-links")} }
                    }
                }
                tbody {
                    for auction in rows {
                        {
                            let forum_thread_url = auction.forum_thread_url;
                            let auction = auction.auction;
                            let ended = auction.settled_at.clone()
                                .or_else(|| auction.effective_end_at.clone())
                                .or_else(|| auction.opened_at.clone())
                                .unwrap_or_else(|| auction.scheduled_open_at.clone());
                            let lots_href = historical_lots_href(&auction.id, &auction.world_id);
                            let bid_log_href = historical_bid_log_href(&auction.id, &auction.world_id);
                            let status_class = auction_status_class(&auction.status);
                            let status_label = historical_auction_status_label(&auction.status);
                            rsx! {
                                tr {
                                    td {
                                        class: "public-auction-rounds__round",
                                        "data-label": t!("auction-history-number"),
                                        "#{auction.sequence}"
                                        span {
                                            class: "public-auction-rounds__status public-auction-rounds__status--{status_class}",
                                            {status_label}
                                        }
                                    }
                                    td { "data-label": t!("auction-history-world"), "{auction_world_label(&auction.world_id)}" }
                                    td { "data-label": t!("auction-history-ended"), Time { value: ended.clone() } }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-lots"), "{auction.lot_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-sold"), "{auction.sold_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-unsold"), "{auction.unsold_count}" }
                                    td { class: "public-auction-rounds__total", "data-label": t!("auction-history-total"), "{format_amount(auction.sold_amount)}" }
                                    td { class: "public-auction-rounds__links", "data-label": t!("auction-history-links"),
                                        Link { to: lots_href, {t!("auction-history-link-lots")} }
                                        Link { to: bid_log_href, {t!("auction-history-link-bidlog")} }
                                        if let Some(forum_thread_url) = forum_thread_url {
                                            a {
                                                href: forum_thread_url,
                                                target: "_blank",
                                                rel: "noopener noreferrer",
                                                {t!("auction-history-link-forum")}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
