//! Admin 结算拍卖选择器与拍卖分页。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::AdminAuctionPage;

use super::super::pagination::AdminPager;
use super::RecoveryConfirmationDraft;
use super::helpers::auction_option_label;

#[component]
pub(super) fn AdminSettlementPicker(
    page: AdminAuctionPage,
    mut selected_auction_id: Signal<String>,
    mut auction_page: Signal<i64>,
    mut lot_page: Signal<i64>,
    mut recovery_page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let current_page = page.pagination.page.max(1);
    let page_size = page.pagination.page_size.max(1);
    rsx! {
        div { class: "admin-panel__actions admin-settlement__picker",
            label {
                {t!("admin-settlement-select-auction")}
                select {
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
                    for auction in page.auctions.clone() {
                        option {
                            value: auction.id.clone(),
                            {auction_option_label(&auction)}
                        }
                    }
                }
            }
        }
        AdminPager {
            page: current_page,
            page_size,
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
