//! Public 历史拍卖列表。

use super::super::*;

#[component]
pub(crate) fn AuctionList() -> Element {
    let query = read_public_query();
    let world_filter = use_signal(|| query.world.clone().unwrap_or_else(|| "all".to_owned()));
    let status_filter = use_signal(|| query.status.clone().unwrap_or_else(|| "settled".to_owned()));
    let page_number = use_signal(|| query.page.max(1));
    let refresh_generation = use_signal(|| 0_u64);
    let worlds = use_resource(fetch_worlds);
    let world_options = match &*worlds.read_unchecked() {
        Some(Ok(worlds)) => enabled_worlds(worlds),
        _ => vec!["main".to_owned(), "isekai".to_owned()],
    };
    let auctions = use_resource(move || {
        let _ = refresh_generation();
        let world = world_filter();
        let status = status_filter();
        let page = page_number();
        let world = (world != "all").then_some(world);
        fetch_auction_page(world, status, page, 20)
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
                status_filter,
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
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
    world_options: Vec<String>,
) -> Element {
    let total = page.total.unwrap_or(page.auctions.len() as i64);
    let page_size = page.page_size.max(1);
    rsx! {
        section { class: "public-auction-page", aria_labelledby: "auction-history-title",
            h1 { id: "auction-history-title", class: "public-auction-category__title",
                {t!("page-auctions-title")}
            }
            p { class: "public-auction-page__status", {t!("page-auctions-note")} }
            div { class: "public-auction-history-controls",
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
                                    query.status = Some(status_filter());
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
                label {
                    {t!("auction-history-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            let status = event.value();
                            status_filter.set(status.clone());
                            page_number.set(1);
                            update_public_query(
                                |query| {
                                    query.status = Some(status.clone());
                                    query.auction_id = None;
                                    query.category = None;
                                    query.page = 1;
                                },
                                false,
                            );
                        },
                        option { value: "settled", {t!("auction-history-status-settled")} }
                        option { value: "final", {t!("auction-history-status-final")} }
                        option { value: "live", {t!("auction-history-status-live")} }
                    }
                }
            }
            if page.auctions.is_empty() {
                Placeholder { title: t!("page-auctions-title"), detail: t!("data-empty") }
            } else {
                AuctionHistoryTable { rows: page.auctions.clone() }
            }
            PublicPager {
                page: page_number(),
                page_size,
                total: Some(total),
                has_more: false,
                label: t!("catalog-pagination"),
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
        div { class: "public-auction-catalog",
            table {
                thead {
                    tr {
                        th { {t!("auction-history-number")} }
                        th { {t!("auction-history-world")} }
                        th { {t!("auction-history-ended")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-lots")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-sold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-unsold")} }
                        th { class: "public-auction-catalog__number", {t!("auction-history-total")} }
                        th { {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for auction in rows {
                        {
                            let ended = auction.settled_at.clone()
                                .or_else(|| auction.effective_end_at.clone())
                                .or_else(|| auction.opened_at.clone())
                                .unwrap_or_else(|| auction.scheduled_open_at.clone());
                            let href = historical_history_href(&auction.id, &auction.world_id);
                            rsx! {
                                tr {
                                    td { class: "public-auction-catalog__number", "#{auction.sequence}" }
                                    td { "{auction_world_label(&auction.world_id)}" }
                                    td { time { datetime: ended.clone(), "{format_auction_time(&ended)}" } }
                                    td { class: "public-auction-catalog__number", "{auction.lot_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.sold_count}" }
                                    td { class: "public-auction-catalog__number", "{auction.unsold_count}" }
                                    td { class: "public-auction-catalog__amount public-auction-catalog__number", "{format_amount(auction.sold_amount)}" }
                                    td {
                                        a { href, {t!("catalog-view")} }
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
