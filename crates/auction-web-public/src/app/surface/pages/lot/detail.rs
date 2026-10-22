//! Public lot 的预览/live 详情和标题组件。

use super::*;
use crate::app::surface::realtime;
use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::format_credit_amount_full;

#[component]
pub(crate) fn PreviewLotDetailView(lot: crate::api::PreviewLotDetail) -> Element {
    let back_href = current_auction_href();
    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot.summary.code.clone(),
                seller: lot.summary.seller_name.clone(),
                status: lot.summary.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard {
                    equipment: EquipmentCardData::from(lot.equipment.clone()),
                    leading: false,
                    watch_control: rsx! {
                        if matches!(lot.summary.status.as_str(), "pending" | "live") {
                            LotWatchButton {
                                lot_id: lot.summary.id.clone(),
                                lot_code: lot.summary.code.clone(),
                            }
                        }
                    },
                }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("catalog-start-price")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.summary.start_price)}" }
                    }
                    p { class: "public-auction-page__status", {t!("lot-not-live")} }
                }
            }
        }
    }
}

#[component]
pub(crate) fn LiveLotDetailView(
    auction_id: String,
    auction_status: String,
    bidding_open: bool,
    lot: crate::api::LiveLot,
) -> Element {
    let back_href = current_auction_href();
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let connection_state = realtime_state();
    let connection_state_name = live_connection_state_name(connection_state);
    let connection_message = realtime::live_connection_message(connection_state);
    // 倒计时的秒针订阅已下沉到 LotDetailCountdown 叶子组件，本组件不再持有每秒变化
    // 的时钟信号，详情页其余部分（竞价历史、操作面板）不随秒针重渲染。
    let mut refresh_generation = use_context::<Signal<u64>>();
    let session = use_context::<PublicSessionResource>();
    let session_view = PublicSessionView::from_snapshot(session.read().as_ref());
    let viewer_player_id = session_view.viewer_player_id.clone();
    let session_ready = session_view.ready;
    let session_error = session_view.error;
    let authenticated = session_view.authenticated;
    let is_leading = viewer_player_id
        .as_deref()
        .zip(lot.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    // 竞价历史页码入 URL（`page` 键）：刷新、分享与浏览器前进/后退都能恢复页码。
    let query_state = use_public_query_state();
    let query_state_signal = query_state.state;
    let bid_page = use_memo(move || query_state_signal().page.max(1));
    let page_size = use_memo(move || query_state_signal().page_size);
    let bid_log = use_resource({
        let auction_id = auction_id.clone();
        let lot_id = lot.id.clone();
        move || {
            let _ = refresh_generation();
            let requested = (bid_page(), page_size());
            let auction_id = auction_id.clone();
            let lot_id = lot_id.clone();
            async move {
                let result = fetch_lot_bid_log(auction_id, lot_id, requested.0, requested.1).await;
                (requested, result)
            }
        }
    });
    use_bid_refresh(bid_log, true);
    let show_countdown =
        lot.status == "live" && matches!(auction_status.as_str(), "live" | "final");
    let loaded = bid_log.read_unchecked();
    let current_bid_log = loaded
        .as_ref()
        .filter(|(requested, _)| *requested == (bid_page(), page_size()))
        .map(|(_, result)| result);

    rsx! {
        section { class: "public-auction-lot", aria_labelledby: "lot-detail-title",
            p {
                aria_live: "polite",
                class: "public-visually-hidden",
                "data-state": connection_state_name,
                {connection_message}
            }
            LotDetailBack { href: back_href }
            LotDetailHeader {
                code: lot_code_for(&lot.equipment.category, lot.lot_number),
                seller: lot.seller_name.clone(),
                status: lot.status.clone(),
            }
            div { class: "public-auction-lot__grid",
                EquipmentCard {
                    equipment: EquipmentCardData::from(lot.equipment.clone()),
                    leading: is_leading,
                    watch_control: rsx! {
                        if auction_status == "live" && lot.status == "live" {
                            LotWatchButton {
                                lot_id: lot.id.clone(),
                                lot_code: lot_code_for(&lot.equipment.category, lot.lot_number),
                            }
                        }
                    },
                }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("lot-current-bid")} }
                        dd {
                            class: "public-auction-lot__price",
                            title: t!("price-exact", amount: format_credit_amount_full(lot.current_price)),
                            "{format_amount(lot.current_price)}"
                        }
                        dt { {t!("lot-next-bid")} }
                        dd {
                            class: "public-auction-lot__price",
                            if lot.next_bid > MAX_PUBLIC_BID_AMOUNT {
                                span { title: t!("bid-limit-note"), {t!("bid-limit-reached")} }
                            } else {
                                span {
                                    title: t!("price-exact", amount: format_credit_amount_full(lot.next_bid)),
                                    "{format_amount(lot.next_bid)}"
                                }
                            }
                        }
                        dt { {t!("lot-leader")} }
                        dd {
                            if let Some(leader) = lot.leader_name.clone() {
                                CatalogParticipant {
                                    name: leader,
                                    marker: None,
                                    trailing: None,
                                }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                        if show_countdown {
                            dt { {t!("lot-ends-in")} }
                            dd {
                                if let Some(ends_at) = lot.effective_end_at.clone() {
                                    LotDetailCountdown { ends_at }
                                } else {
                                    span { class: "public-auction-lot__none", {t!("value-empty")} }
                                }
                            }
                        }
                    }
                    if session_error {
                        p { role: "alert", {t!("session-error")} }
                    }
                    // 拍卖或拍品结束后不再展示出价、代理，装备分组行的标记沿用同一状态门禁。
                    if auction_status == "live" && lot.status == "live" {
                        LiveCatalogActions {
                            lot_id: lot.id.clone(),
                            lot_code: lot_code_for(&lot.equipment.category, lot.lot_number),
                            can_bid: bidding_open && lot.status == "live",
                            current_price: lot.current_price,
                            next_bid: lot.next_bid,
                            lot_status: lot.status.clone(),
                            allow_slow: auction_status == "live",
                            session_ready,
                            block_ordinary_for_active_proxy: true,
                            authenticated,
                            leading: is_leading,
                            show_view: false,
                            show_proxy_summary: true,
                        }
                    }
                }
            }
            section { class: "public-auction-lot__history", aria_labelledby: "lot-bid-history-title",
                h2 { id: "lot-bid-history-title", {t!("lot-bid-history")} }
                match current_bid_log {
                    None => rsx! { p { role: "status", {t!("data-loading")} } },
                    Some(Err(_)) => rsx! {
                        div { class: "public-auction-page__status", role: "alert",
                            p { {t!("data-error")} }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    refresh_generation
                                        .set(refresh_generation().saturating_add(1));
                                },
                                {t!("data-retry")}
                            }
                        }
                    },
                    Some(Ok(page)) if !public_page_matches(page.page, page.page_size, bid_page(), page_size()) => rsx! {
                        p { role: "status", {t!("data-loading")} }
                    },
                    Some(Ok(page)) => rsx! {
                        if page.bids.is_empty() {
                            p { class: "public-auction-lot__none", {t!("lot-no-bids")} }
                        } else {
                            BidHistoryTable { page: page.clone() }
                        }
                        PublicPager {
                            page: bid_page(),
                            page_size: page.page_size,
                            total: page.total,
                            has_more: page.has_more.unwrap_or(false),
                            label: t!("lot-bid-history"),
                            on_page_size_change: move |size| update_public_page_size(query_state_signal, size),
                            on_page_clamp: move |next_page| {
                                update_public_query_state(
                                    query_state_signal,
                                    |query| query.page = next_page,
                                    QueryWriteMode::Replace,
                                );
                            },
                            on_page_change: move |next_page| {
                                update_public_query_state(
                                    query_state_signal,
                                    |query| query.page = next_page,
                                    QueryWriteMode::Push,
                                );
                            },
                        }
                    },
                }
            }
        }
    }
}

/// lot 详情的结束倒计时。秒级节拍只在本叶子组件内订阅，详情页其余部分不随秒针重渲染。
#[component]
fn LotDetailCountdown(ends_at: String) -> Element {
    let tick = use_second_tick();
    let _ = tick();
    let countdown = countdown_display(&ends_at, estimated_now());
    rsx! {
        time { datetime: ends_at.clone(), class: countdown_class(&countdown), "{countdown.label}" }
    }
}

#[component]
pub(crate) fn LotDetailBack(href: String) -> Element {
    rsx! {
        p { class: "public-auction-lot__back",
            Link { to: href, "← ", {t!("lot-back-to-auction")} }
        }
    }
}

#[component]
pub(crate) fn LotDetailHeader(code: String, seller: String, status: String) -> Element {
    rsx! {
        header { class: "public-auction-lot__header",
            h1 { id: "lot-detail-title", "{code}" }
            span { class: "public-auction-status public-auction-status--{lot_status_class(&status)}", "{lot_status_label(&status)}" }
            span { class: "public-auction-lot__seller",
                {t!("catalog-seller")}, ": ", "{seller}"
            }
        }
    }
}
