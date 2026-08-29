//! Public lot 的预览/live 详情和标题组件。

use super::*;
use crate::app::surface::realtime;

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
                EquipmentCard { equipment: EquipmentCardData::from(lot.equipment.clone()) }
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
    let mut clock = use_signal(estimated_now);
    let _clock_task = use_future(move || async move {
        loop {
            TimeoutFuture::new(1_000).await;
            clock.set(estimated_now());
        }
    });
    let mut refresh_generation = use_context::<Signal<u64>>();
    let session = use_context::<PublicSessionResource>();
    let session_snapshot = session.read().clone();
    let viewer_name = match session_snapshot.as_ref() {
        Some(Ok(Some(session))) if session.authenticated => Some(session.username.clone()),
        _ => None,
    };
    let session_ready = matches!(session_snapshot.as_ref(), Some(Ok(_)));
    let session_error = matches!(session_snapshot.as_ref(), Some(Err(_)));
    let authenticated = matches!(
        session_snapshot.as_ref(),
        Some(Ok(Some(session))) if session.authenticated
    );
    let is_leading = viewer_name
        .as_deref()
        .zip(lot.leader_name.as_deref())
        .is_some_and(|(viewer, leader)| viewer == leader);
    let mut bid_page = use_signal(|| 1_i64);
    let bid_log = use_resource({
        let auction_id = auction_id.clone();
        let lot_id = lot.id.clone();
        move || {
            let _ = refresh_generation();
            let page = bid_page();
            fetch_lot_bid_log(auction_id.clone(), lot_id.clone(), page)
        }
    });
    let countdown =
        lot.effective_end_at.as_deref().map(|ends_at| countdown_display(ends_at, clock()));

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
                EquipmentCard { equipment: EquipmentCardData::from(lot.equipment.clone()) }
                div { class: "public-auction-lot__bidding",
                    dl {
                        dt { {t!("lot-current-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.current_price)}" }
                        dt { {t!("lot-next-bid")} }
                        dd { class: "public-auction-lot__price", "{format_amount(lot.next_bid)}" }
                        dt { {t!("lot-leader")} }
                        dd {
                            if let Some(leader) = lot.leader_name.clone() {
                                CatalogParticipant {
                                    name: leader,
                                    marker: None,
                                    trailing: is_leading.then(|| t!("catalog-you")),
                                }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                        dt { {t!("lot-ends-in")} }
                        dd {
                            if let (Some(ends_at), Some(countdown)) =
                                (lot.effective_end_at.clone(), countdown)
                            {
                                time { datetime: ends_at, class: countdown_class(&countdown), "{countdown.label}" }
                            } else {
                                span { class: "public-auction-lot__none", {t!("value-empty")} }
                            }
                        }
                    }
                    if session_error {
                        p { role: "alert", {t!("session-error")} }
                    }
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
            section { class: "public-auction-lot__history", aria_labelledby: "lot-bid-history-title",
                h2 { id: "lot-bid-history-title", {t!("lot-bid-history")} }
                match &*bid_log.read_unchecked() {
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
                    Some(Ok(page)) if page.bids.is_empty() => rsx! {
                        p { class: "public-auction-lot__none", {t!("lot-no-bids")} }
                    },
                    Some(Ok(page)) => rsx! {
                        BidHistoryTable { page: page.clone() }
                        PublicPager {
                            page: bid_page(),
                            page_size: page.page_size,
                            total: page.total,
                            has_more: page.has_more.unwrap_or(false),
                            label: t!("lot-bid-history"),
                            on_page_change: move |next_page| bid_page.set(next_page),
                        }
                    },
                }
            }
        }
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
