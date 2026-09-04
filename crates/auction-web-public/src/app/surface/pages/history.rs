//! Public 历史拍卖列表。

use super::super::*;

#[component]
pub(crate) fn AuctionList() -> Element {
    let query = read_public_query();
    let world_filter = use_signal(|| query.world.clone().unwrap_or_else(|| "all".to_owned()));
    let status_filter = query
        .status
        .as_deref()
        .filter(|status| matches!(*status, "live" | "final" | "settled"))
        .unwrap_or("settled")
        .to_owned();
    let page_number = use_signal(|| query.page.max(1));
    let refresh_generation = use_signal(|| 0_u64);
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => supported_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let auctions = use_resource({
        let status_filter = status_filter.clone();
        move || {
            let _ = refresh_generation();
            let world = world_filter();
            let status = status_filter.clone();
            let page = page_number();
            let world = (world != "all").then_some(world);
            fetch_auction_page(world, status, page, 10)
        }
    });
    match &*auctions.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-auctions-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            super::shared::RetryPlaceholder {
                title: t!("page-auctions-title"),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            AuctionHistoryPage {
                page: page.clone(),
                world_filter,
                page_number,
                world_options,
            }
        },
    }
}

#[component]
pub(crate) fn AuctionHistoryPage(
    page: AuctionListPage,
    mut world_filter: Signal<String>,
    mut page_number: Signal<i64>,
    world_options: Vec<String>,
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
                        value: world_filter(),
                        onchange: move |event| {
                            let world = event.value();
                            world_filter.set(world.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.world = (world != "all").then_some(world.clone());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
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
                AuctionHistoryTable { rows: page.auctions.clone() }
            }
            PublicPager {
                page: page_number(),
                page_size,
                total: page.total,
                has_more: page.has_more.unwrap_or(false),
                label: t!("page-auctions-title"),
                on_page_change: move |page| {
                    page_number.set(page);
                    update_public_query(|query| query.page = page, false);
                },
            }
        }
    }
}

#[component]
pub(crate) fn AuctionHistoryTable(rows: Vec<HistoricalAuction>) -> Element {
    rsx! {
        div { class: "public-auction-rounds__table",
            table {
                caption { class: "public-visually-hidden", {t!("page-auctions-title")} }
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
                                    td { "data-label": t!("auction-history-ended"), time { datetime: ended.clone(), "{format_auction_time(&ended)}" } }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-lots"), "{auction.lot_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-sold"), "{auction.sold_count}" }
                                    td { class: "public-auction-rounds__number", "data-label": t!("auction-history-unsold"), "{auction.unsold_count}" }
                                    td { class: "public-auction-rounds__total", "data-label": t!("auction-history-total"), "{format_amount(auction.sold_amount)}" }
                                    td { class: "public-auction-rounds__links", "data-label": t!("auction-history-links"),
                                        Link { to: lots_href, {t!("auction-history-link-lots")} }
                                        Link { to: bid_log_href, {t!("auction-history-link-bidlog")} }
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
