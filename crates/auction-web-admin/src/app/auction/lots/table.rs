//! Admin lot 表格和页面级分页编排。

use dioxus::prelude::*;
use dioxus_i18n::t;

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
        section { class: "admin-panel", aria_labelledby: "admin-lots-title",
            h1 { id: "admin-lots-title",
                {t!("page-auction-lots-title", auction: page.auction.sequence)}
            }
            p { class: "admin-lots__back",
                Link { to: Route::Auctions {}, {t!("admin-lots-back")} }
            }
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lot_update_notice_label(notice)}
                }
            }
            if page.auction.status == "draft" && page.capabilities.manual_intake {
                AdminIntakeForm {
                    world_id: page.auction.world_id.clone(),
                    reload_generation,
                }
            }
            if page.lots.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-lots-empty")} }
            } else {
                div { class: "public-auction-catalog admin-lots__table-wrap",
                    table {
                        thead {
                            tr {
                                th { class: "admin-lots__col-code", {t!("admin-lots-code")} }
                                th { class: "admin-lots__col-equipment", {t!("admin-lots-equipment")} }
                                th { class: "admin-lots__col-seller", {t!("admin-lots-seller")} }
                                th { class: "admin-lots__col-submission", {t!("admin-lots-submission")} }
                                th { class: "admin-lots__col-status", {t!("admin-lots-status")} }
                                th { class: "admin-lots__col-start", {t!("admin-lots-start-price")} }
                                th { class: "admin-lots__col-current", {t!("admin-lots-price")} }
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
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("admin-lots-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
            if let Some((lot_id, lot_number)) = selected_lot() {
                AdminProxyHistory { lot_id, lot_number, selected_lot, reload_generation }
            }
        }
    }
}
