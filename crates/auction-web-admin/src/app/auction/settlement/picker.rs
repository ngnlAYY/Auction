//! Admin 结算拍卖选择器与拍卖分页。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminAuctionPage;
use crate::app::surface::AdminActionFeedback;

use super::super::pagination::AdminPager;
use super::RecoveryConfirmationDraft;
use super::helpers::auction_option_label;

#[component]
pub(super) fn AdminSettlementPicker(
    page: Option<AdminAuctionPage>,
    mut selected_auction_id: Signal<String>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    rsx! {
        div { class: "admin-auction-filter",
            label { r#for: "settlement-auction",
                {t!("admin-settlement-select-auction")}
                select {
                    id: "settlement-auction",
                    class: "admin-auction-settlement-select",
                    value: selected_auction_id(),
                    onchange: move |event| {
                        selected_auction_id.set(event.value());
                        lot_page.set(1);
                        recovery_page.set(1);
                        confirmation.set(None);
                        reason.set(String::new());
                        action_feedback.set(None);
                    },
                    option { value: "", {t!("admin-settlement-choose-auction")} }
                    if let Some(page) = page {
                        for auction in page.auctions {
                            option {
                                value: auction.id.clone(),
                                {auction_option_label(&auction)}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AdminSettlementAuctionPager(
    page: AdminAuctionPage,
    mut selected_auction_id: Signal<String>,
    mut auction_page: Signal<i64>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
) -> Element {
    rsx! {
        AdminPager {
            page: page.pagination.page.max(1),
            page_size: page.pagination.page_size.max(1),
            total: page.pagination.total,
            label: t!("admin-settlement-auction-pagination"),
            on_page_change: move |next_page| {
                auction_page.set(next_page);
                selected_auction_id.set(String::new());
                lot_page.set(1);
                recovery_page.set(1);
            },
        }
    }
}
