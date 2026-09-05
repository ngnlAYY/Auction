//! Admin lot 表格和页面级分页编排。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager};

use crate::api::AdminLotPage;

use super::super::Route;
use super::super::intake::AdminIntakeForm;
use super::helpers::lot_update_notice_label;
use super::proxy::AdminProxyHistory;
use super::row::AdminLotRow;
use super::state::LotUpdateNotice;

#[component]
pub(super) fn AdminLotTable(
    page: AdminLotPage,
    /// 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。
    on_page_change: EventHandler<i64>,
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
            if let Some(notice) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status",
                    {lot_update_notice_label(notice)}
                }
            }
            section { class: "admin-auction-lot-section", aria_labelledby: "lot-title",
                h2 { id: "lot-title", class: "admin-visually-hidden",
                    {t!("page-auction-lots-title", auction: page.auction.sequence)}
                }
                if matches!(page.auction.status.as_str(), "draft" | "preview")
                    && page.capabilities.manual_intake
                {
                    AdminIntakeForm {
                        world_id: page.auction.world_id.clone(),
                        reload_generation,
                    }
                }
                div { class: "admin-auction-table-wrap admin-auction-lot-table-wrap",
                    table { class: "admin-auction-table admin-auction-lot-table ui-table--responsive",
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
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                AdminLotRow { key: "{lot.id}", lot, selected_lot, notice, reload_generation, allow_lot_update }
                            }
                        }
                    }
                }
                if page.lots.is_empty() {
                    AsyncState {
                        kind: AsyncStateKind::Empty,
                        title: t!("admin-lots-empty"),
                        detail: "",
                    }
                }
                Pager {
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: Some(page.pagination.total),
                    label: t!("admin-lots-pagination"),
                    previous_label: t!("admin-pagination-previous"),
                    status: move |(page, pages): (i64, i64)| {
                        t!("admin-pagination-status", page: page, pages: pages)
                    },
                    next_label: t!("admin-pagination-next"),
                    jump_label: t!("admin-pagination-input"),
                    jump_button_label: t!("admin-pagination-go"),
                    on_page_change,
                }
            }
            if let Some((lot_id, lot_code)) = selected_lot() {
                AdminProxyHistory { lot_id, lot_code, selected_lot, reload_generation }
            }
        }
    }
}
