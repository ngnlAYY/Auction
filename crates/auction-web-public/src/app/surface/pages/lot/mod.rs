//! Public lot 详情页面装配。

use super::super::*;

mod detail;
mod equipment;
mod history;

pub(crate) use detail::{LiveLotDetailView, LotDetailBack, LotDetailHeader, PreviewLotDetailView};
pub(crate) use equipment::EquipmentCard;
pub(crate) use history::{BidHistoryTable, bid_mechanism_label};

#[component]
pub(crate) fn LotDetail(lot_code: String) -> Element {
    let mut refresh_generation = use_context::<Signal<u64>>();
    let realtime_state = use_context::<Signal<LiveConnectionState>>();
    let _fallback_poll = use_future(move || async move {
        loop {
            TimeoutFuture::new(5_000).await;
            if realtime_state() != LiveConnectionState::Connected {
                refresh_generation.set(refresh_generation().saturating_add(1));
            }
        }
    });
    let lot_id = lot_code.clone();
    let detail = use_resource(move || {
        let _ = refresh_generation();
        let lot_id = lot_id.clone();
        async move { fetch_lot_detail(lot_id).await }
    });

    match &*detail.read_unchecked() {
        None => rsx! {
            Placeholder { title: t!("page-lot-title", code: lot_code.clone()), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            super::shared::RetryPlaceholder {
                title: t!("page-lot-title", code: lot_code.clone()),
                detail: t!("data-error"),
                refresh_generation,
            }
        },
        Some(Ok(page)) => match page.lot.as_ref() {
            Some(LotDetailItem::Preview(lot)) => rsx! {
                PreviewLotDetailView { lot: lot.clone() }
            },
            Some(LotDetailItem::Live(lot)) => rsx! {
                LiveLotDetailView {
                    auction_id: page.auction_id.clone(),
                    auction_status: page.auction_status.clone(),
                    bidding_open: page.bidding_open,
                    lot: lot.clone(),
                }
            },
            None => rsx! {
                Placeholder { title: t!("page-lot-title", code: lot_code), detail: t!("data-empty") }
            },
        },
    }
}
