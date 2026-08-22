//! Public 只读历史页面。
//!
//! 这些页面只消费稳定 HTTP DTO；分页游标和统计语义仍由 Public API 决定，浏览器不重建业务
//! 状态，也不把数据库模型带入组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    BidLogEntry, BidLogPage, PriceHistoryEntry, PriceHistoryPage, PriceHistoryStats,
    fetch_current_bid_log, fetch_price_history,
};

use super::Placeholder;

pub(super) fn bid_log() -> Element {
    let bid_log = use_resource(fetch_current_bid_log);
    match &*bid_log.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-bidlog-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-bidlog-title"), detail: t!("data-error") }
        },
        Some(Ok(None)) => rsx! {
            Placeholder { title: t!("page-bidlog-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(page))) if page.bids.is_empty() => rsx! {
            Placeholder { title: t!("page-bidlog-title"), detail: t!("data-empty") }
        },
        Some(Ok(Some(page))) => rsx! {
            BidLogTable { page: page.clone() }
        },
    }
}

#[component]
fn BidLogTable(page: BidLogPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "bid-log-title",
            h1 { id: "bid-log-title", class: "public-auction-category__title",
                {t!("page-bidlog-title")}
            }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("bidlog-lot")} }
                            th { {t!("bidlog-bidder")} }
                            th { {t!("bidlog-amount")} }
                            th { {t!("bidlog-mechanism")} }
                            th { {t!("bidlog-valid")} }
                            th { {t!("bidlog-time")} }
                        }
                    }
                    tbody {
                        for bid in page.bids {
                            BidLogRow { bid }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn BidLogRow(bid: BidLogEntry) -> Element {
    let validity = if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") };
    rsx! {
        tr {
            td { class: "public-auction-catalog__number", "{bid.lot_number}" }
            td { "{bid.bidder_name}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "{bid.amount}" }
            td { "{bid.mechanism}" }
            td { "{validity}" }
            td {
                time { datetime: bid.created_at.clone(), "{bid.created_at}" }
            }
        }
    }
}

pub(super) fn history() -> Element {
    let history = use_resource(fetch_price_history);
    match &*history.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-history-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            Placeholder { title: t!("page-history-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) if page.rows.is_empty() => rsx! {
            Placeholder { title: t!("page-history-title"), detail: t!("data-empty") }
        },
        Some(Ok(page)) => rsx! {
            PriceHistoryTable { page: page.clone() }
        },
    }
}

#[component]
fn PriceHistoryTable(page: PriceHistoryPage) -> Element {
    rsx! {
        section { class: "public-auction-category", aria_labelledby: "price-history-title",
            h1 { id: "price-history-title", class: "public-auction-category__title",
                {t!("page-history-title")}
            }
            if let Some(stats) = page.stats.clone() {
                PriceHistoryStatsView { stats }
            }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("history-world")} }
                            th { {t!("history-round")} }
                            th { {t!("history-item")} }
                            th { {t!("history-buyer")} }
                            th { {t!("history-seller")} }
                            th { {t!("history-price")} }
                            th { {t!("history-closed")} }
                        }
                    }
                    tbody {
                        for row in page.rows {
                            PriceHistoryRow { row }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PriceHistoryStatsView(stats: PriceHistoryStats) -> Element {
    rsx! {
        dl { class: "public-auction-preview-stats", aria_label: t!("history-statistics"),
            div {
                dt { {t!("history-count")} }
                dd { "{stats.count}" }
            }
            div {
                dt { {t!("history-min")} }
                dd { "{stats.min}" }
            }
            div {
                dt { {t!("history-max")} }
                dd { "{stats.max}" }
            }
            div {
                dt { {t!("history-median")} }
                dd { "{stats.median}" }
            }
        }
    }
}

#[component]
fn PriceHistoryRow(row: PriceHistoryEntry) -> Element {
    rsx! {
        tr {
            td { "{row.world_id}" }
            td { class: "public-auction-catalog__number", "{row.auction_sequence}" }
            td { "{row.equipment_name}" }
            td { "{row.buyer_name}" }
            td { "{row.seller_name}" }
            td { class: "public-auction-catalog__amount public-auction-catalog__number", "{row.price}" }
            td {
                time { datetime: row.closed_at.clone(), "{row.closed_at}" }
            }
        }
    }
}
