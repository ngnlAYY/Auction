//! Admin lot 表格和页面级分页编排。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminLotPage, fetch_admin_auctions};

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
    let selected_lot = use_signal(|| None::<(String, String)>);
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
            if page.auction.status == "preview" {
                PreviewIntakeHandoff { world_id: page.auction.world_id.clone() }
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
                div { class: "admin-auction-table-wrap admin-auction-lot-table-wrap",
                    table { class: "admin-auction-table admin-auction-lot-table",
                        caption { class: "admin-visually-hidden",
                            {t!("page-auction-lots-title", auction: page.auction.sequence)}
                        }
                        thead {
                            tr {
                                th { scope: "col", class: "admin-auction-lot-code", {t!("admin-lots-code")} }
                                th { scope: "col", class: "admin-auction-lot-equipment", {t!("admin-lots-equipment")} }
                                th { scope: "col", class: "admin-auction-lot-seller", {t!("admin-lots-seller")} }
                                th { scope: "col", class: "admin-auction-lot-submission", {t!("admin-lots-submission")} }
                                th { scope: "col", class: "admin-auction-lot-status", {t!("admin-lots-status")} }
                                th { scope: "col", class: "admin-auction-lot-start", {t!("admin-lots-start-price")} }
                                th { scope: "col", class: "admin-auction-lot-current", {t!("admin-lots-price")} }
                                th { scope: "col", class: "admin-auction-lot-actions", {t!("admin-lots-actions")} }
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
            if let Some((lot_id, lot_code)) = selected_lot() {
                AdminProxyHistory { lot_id, lot_code, selected_lot, reload_generation }
            }
        }
    }
}

#[component]
fn PreviewIntakeHandoff(world_id: String) -> Element {
    let drafts = use_resource({
        let world_id = world_id.clone();
        move || fetch_admin_auctions(world_id.clone(), "draft".to_owned(), 1)
    });
    let successor_id = drafts
        .read()
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|page| page.auctions.first())
        .map(|auction| auction.id.clone());

    rsx! {
        aside { class: "admin-auction-intro", role: "status",
            p { {t!("admin-lots-preview-handoff")} }
            if let Some(auction_id) = successor_id {
                Link {
                    to: Route::AuctionLots { auction_id },
                    {t!("admin-lots-open-successor")}
                }
            } else {
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
        }
    }
}
