//! Admin lot 表格和页面级分页编排。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminLotPage;

use super::super::Route;
use super::super::intake::AdminIntakeForm;
use super::super::pagination::AdminPager;
use super::helpers::lot_update_notice_label;
use super::proxy::AdminProxyHistory;
use super::row::AdminLotRow;
use super::state::LotUpdateNotice;

#[component]
pub(super) fn AdminLotTable(
    page: AdminLotPage,
    mut page_number: Signal<i64>,
    reload_generation: Signal<u64>,
) -> Element {
    let selected_lot = use_signal(|| None::<(String, i32)>);
    let notice = use_signal(|| None::<LotUpdateNotice>);
    let allow_lot_update = page.auction.status == "draft";

    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "auction-title",
            h1 { id: "auction-title",
                {t!("page-auction-lots-title", auction: page.auction.sequence)}
            }
            p { class: "admin-auction-intro",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lot_update_notice_label(notice)}
                }
            }
            section { class: "admin-auction-lot-section", aria_labelledby: "lot-title",
                h2 { id: "lot-title", class: "admin-visually-hidden",
                    {t!("page-auction-lots-title", auction: page.auction.sequence)}
                }
                if page.auction.status == "draft" && page.capabilities.manual_intake {
                    AdminIntakeForm {
                        world_id: page.auction.world_id.clone(),
                        reload_generation,
                    }
                }
                div { class: "admin-auction-table-wrap",
                    table { class: "admin-auction-table admin-auction-lot-table",
                        thead {
                            tr {
                                th { class: "admin-auction-lot-code", {t!("admin-lots-code")} }
                                th { class: "admin-auction-lot-equipment", {t!("admin-lots-equipment")} }
                                th { class: "admin-auction-lot-seller", {t!("admin-lots-seller")} }
                                th { class: "admin-auction-lot-submission", {t!("admin-lots-submission")} }
                                th { class: "admin-auction-lot-status", {t!("admin-lots-status")} }
                                th { class: "admin-auction-lot-start", {t!("admin-lots-start-price")} }
                                th { class: "admin-auction-lot-current", {t!("admin-lots-price")} }
                                th { {t!("admin-lots-actions")} }
                            }
                        }
                        tbody {
                            for lot in page.lots.clone() {
                                AdminLotRow { lot, selected_lot, notice, reload_generation, allow_lot_update }
                            }
                        }
                    }
                }
                if page.lots.is_empty() {
                    p { class: "admin-auction-status", {t!("admin-lots-empty")} }
                }
                AdminPager {
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: page.pagination.total,
                    label: t!("admin-lots-pagination"),
                    on_page_change: move |next_page| page_number.set(next_page),
                }
            }
            if let Some((lot_id, lot_number)) = selected_lot() {
                AdminProxyHistory { lot_id, lot_number, selected_lot, reload_generation }
            }
        }
    }
}
