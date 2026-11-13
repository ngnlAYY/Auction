//! 详情抽屉里的竞价历史：最近几条紧凑记录，以及按页浏览全部记录的弹窗。
//! 两者都只读公开竞价记录；出价成功与实时竞价通知触发同一刷新节拍。

use auction_web_shared::{Dialog, Time, format_credit_amount as format_amount, legacy_t as t};
use dioxus::prelude::*;

use crate::api::fetch_lot_bid_log;
use crate::surface::{BidHistoryParticipant, PublicPager, use_bid_refresh};

/// 抽屉内直接展示的最近出价条数。
const LATEST_BIDS: i64 = 4;
/// 完整历史弹窗的固定页长。弹窗不提供页长选择，也不读写地址栏分页参数。
const HISTORY_PAGE_SIZE: i64 = 10;

/// 最近出价。`return_focus` 为真时“查看全部”带 autofocus：完整历史弹窗关闭后焦点回到它。
#[component]
pub(super) fn DetailLatestBids(
    auction_id: String,
    lot_id: String,
    return_focus: bool,
    on_view_all: EventHandler<()>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut bids =
        use_resource(use_reactive((&auction_id, &lot_id), move |(auction_id, lot_id)| {
            // 本人命令成功后推进的刷新代数；他人出价由下方的竞价通知补读。
            let _ = refresh_generation();
            async move { fetch_lot_bid_log(auction_id, lot_id, 1, LATEST_BIDS).await }
        }));
    use_bid_refresh(bids, true);
    let snapshot = bids.read_unchecked();
    // 分页模式的响应带总数；缺失时不提供“查看全部”，不猜测条数。
    let total = match snapshot.as_ref() {
        Some(Ok(page)) => page.total.unwrap_or_default(),
        _ => 0,
    };

    rsx! {
        section { class: "public-catalog-detail__section",
            div { class: "public-catalog-detail__section-head",
                h3 { class: "public-catalog-detail__section-title", {t!("lot-bid-history")} }
                if total > LATEST_BIDS {
                    button {
                        class: "public-catalog-detail__more",
                        r#type: "button",
                        aria_haspopup: "dialog",
                        autofocus: return_focus,
                        onclick: move |_| on_view_all.call(()),
                        {t!("catalog-detail-view-all", count: total.to_string())}
                    }
                }
            }
            // 读取中不放“加载中”文字，避免内容到达时把区块撑开又收回。
            {match snapshot.as_ref() {
                None => rsx! {},
                Some(Err(_)) => rsx! {
                    p { class: "public-catalog-detail__status", role: "alert",
                        span { {t!("data-error")} }
                        button {
                            class: "public-catalog-detail__more",
                            r#type: "button",
                            onclick: move |_| bids.restart(),
                            {t!("data-retry")}
                        }
                    }
                },
                Some(Ok(page)) if page.bids.is_empty() => rsx! {
                    p { class: "public-catalog-detail__empty", {t!("lot-no-bids")} }
                },
                Some(Ok(page)) => rsx! {
                    ol { class: "public-catalog-detail__bids",
                        for bid in page.bids.iter().cloned() {
                            li {
                                key: "{bid.id}",
                                class: if bid.valid {
                                    "public-catalog-detail__bid-row public-catalog-detail__bid-row--leading"
                                } else {
                                    "public-catalog-detail__bid-row"
                                },
                                "aria-description": if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") },
                                span { class: "public-catalog-detail__bidder",
                                    BidHistoryParticipant {
                                        name: bid.bidder_name,
                                        source: bid.source,
                                        source_post_number: bid.source_post_number,
                                        source_post_url: bid.source_post_url,
                                    }
                                }
                                b { class: "public-catalog-detail__bid-amount", "{format_amount(bid.amount)}" }
                                span { class: "public-catalog-detail__bid-time", Time { value: bid.created_at } }
                            }
                        }
                    }
                },
            }}
        }
    }
}

/// 完整竞价历史弹窗。作为抽屉的兄弟节点渲染（不嵌套在抽屉内），焦点陷阱把它视为最顶层；
/// 关闭后焦点回到抽屉里打开它的按钮。分页状态只存在于弹窗内。
#[component]
pub(super) fn DetailBidHistoryDialog(
    auction_id: String,
    lot_id: String,
    lot_code: String,
    lot_name: String,
    on_close: EventHandler<()>,
) -> Element {
    let refresh_generation = use_context::<Signal<u64>>();
    let mut page = use_signal(|| 1_i64);
    let bid_log =
        use_resource(use_reactive((&auction_id, &lot_id), move |(auction_id, lot_id)| {
            let _ = refresh_generation();
            let requested = page();
            async move {
                let result =
                    fetch_lot_bid_log(auction_id, lot_id, requested, HISTORY_PAGE_SIZE).await;
                (requested, result)
            }
        }));
    use_bid_refresh(bid_log, true);
    let snapshot = bid_log.read_unchecked();
    // 翻页期间保留上一页的表格，新页到达后整体替换；只有首次读取时正文为空。
    let loaded = snapshot.as_ref().map(|(_, result)| result);

    rsx! {
        Dialog {
            title: t!("lot-bid-history"),
            close_label: t!("catalog-close"),
            on_close: move |_| on_close.call(()),
            div { class: "public-bid-dialog__lot public-catalog-history__lot",
                div { class: "public-bid-dialog__lot-name",
                    span { class: "public-bid-dialog__lot-code", "{lot_code}" }
                    span { class: "public-bid-dialog__lot-title", "{lot_name}" }
                }
            }
            div { class: "public-catalog-history",
                {match loaded {
                    None => rsx! {},
                    Some(Err(_)) => rsx! {
                        p { class: "public-catalog-detail__status", role: "alert", {t!("data-error")} }
                    },
                    Some(Ok(log)) if log.bids.is_empty() => rsx! {
                        p { class: "public-catalog-detail__empty", {t!("lot-no-bids")} }
                    },
                    Some(Ok(log)) => rsx! {
                        BidHistoryTable { page: log.clone() }
                        PublicPager {
                            page: log.page.unwrap_or(1),
                            page_size: HISTORY_PAGE_SIZE,
                            total: log.total,
                            has_more: log.has_more.unwrap_or(false),
                            hide_when_single: true,
                            show_size_picker: false,
                            label: t!("lot-bid-history"),
                            on_page_change: move |next| page.set(next),
                            on_page_clamp: move |next| page.set(next),
                        }
                    },
                }}
            }
        }
    }
}

/// 完整历史弹窗里的一页竞价记录：竞价者、金额与时间；有效出价整行标出。
#[component]
fn BidHistoryTable(page: crate::api::BidLogPage) -> Element {
    rsx! {
        div { class: "public-auction-lot__table",
            table {
                aria_label: t!("page-bidlog-title"),
                auction_web_shared::TableColumns { layout: "public-lot-bid-history", widths: vec![38, 22, 40] }
                thead {
                    tr {
                        th { {t!("bidlog-bidder")} }
                        th { class: "public-auction-lot__number", {t!("bidlog-amount")} }
                        th { {t!("bidlog-time")} }
                    }
                }
                tbody {
                    for bid in page.bids {
                        tr {
                            class: if bid.valid { "public-auction-lot__leading" } else { "" },
                            "aria-description": if bid.valid { t!("bidlog-valid") } else { t!("bidlog-invalid") },
                            td { class: "public-auction-lot__participant-cell", "data-label": t!("bidlog-bidder"),
                                BidHistoryParticipant {
                                    name: bid.bidder_name,
                                    source: bid.source,
                                    source_post_number: bid.source_post_number,
                                    source_post_url: bid.source_post_url,
                                }
                            }
                            td { class: "public-auction-lot__amount public-auction-lot__number", "data-label": t!("bidlog-amount"), "{format_amount(bid.amount)}" }
                            td { "data-label": t!("bidlog-time"), Time { value: bid.created_at } }
                        }
                    }
                }
            }
        }
    }
}
