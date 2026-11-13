//! Admin lot 表格和页面级分页编排。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::surface::pagination::{AdminPager as Pager, AdminPagination};
use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminLot, AdminLotPage};

use super::super::Route;
use super::super::intake::AdminIntakeForm;
use super::super::sort::{SortDirection, SortableTh, sort_rows};
use super::helpers::lot_update_notice_label;
use super::proxy::AdminProxyHistory;
use super::row::AdminLotRow;
use super::state::LotUpdateNotice;

#[component]
pub(super) fn AdminLotTable(
    page: AdminLotPage,
    pagination: AdminPagination,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    /// 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。
    on_page_change: EventHandler<i64>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    reload_generation: Signal<u64>,
) -> Element {
    let selected_lot = use_signal(|| None::<(String, String)>);
    let notice = use_signal(|| None::<LotUpdateNotice>);
    let allow_lot_update = page.auction.status == "draft";
    let is_empty = page.lots.is_empty();
    let mut lots = page.lots;
    sort_lots(&mut lots, &sort, direction);

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
            section { class: "admin-auction-lot-section",
                if matches!(page.auction.status.as_str(), "draft" | "preview")
                    && page.capabilities.manual_intake
                {
                    for world_id in [page.auction.world_id] {
                        AdminIntakeForm {
                            key: "{world_id}",
                            world_id,
                        }
                    }
                }
                div { class: "admin-auction-table-wrap admin-auction-lot-table-wrap",
                    table { class: "admin-auction-table admin-auction-lot-table",
                        aria_label: t!("page-auction-lots-title", auction: page.auction.sequence),
                        auction_web_shared::TableColumns { layout: "admin-lots", widths: vec![7, 28, 10, 7, 9, 9, 12, 18] }
                        thead {
                            tr {
                                SortableTh {
                                    column: "code",
                                    label: t!("admin-lots-code"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-lot-code",
                                }
                                th { scope: "col", class: "admin-auction-lot-equipment", {t!("admin-lots-equipment")} }
                                th { scope: "col", class: "admin-auction-lot-seller", {t!("admin-lots-seller")} }
                                th { scope: "col", class: "admin-auction-lot-submission", {t!("admin-lots-submission")} }
                                SortableTh {
                                    column: "status",
                                    label: t!("admin-lots-status"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-lot-status",
                                }
                                SortableTh {
                                    column: "start_price",
                                    label: t!("admin-lots-start-price"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-lot-start",
                                }
                                SortableTh {
                                    column: "current_price",
                                    label: t!("admin-lots-price"),
                                    active: sort.clone(),
                                    direction,
                                    on_sort: on_sort_change,
                                    class: "admin-auction-lot-current",
                                }
                                th { scope: "col", class: "admin-auction-lot-actions", {t!("admin-lots-actions")} }
                            }
                        }
                        tbody {
                            for lot in lots {
                                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                                AdminLotRow { key: "{lot.id}", lot, selected_lot, notice, allow_lot_update }
                            }
                        }
                    }
                }
                if is_empty {
                    AsyncState {
                        kind: AsyncStateKind::Empty,
                        title: t!("admin-lots-empty"),
                        detail: "",
                    }
                }
                Pager {
                    pagination,
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: Some(page.pagination.total),
                    label: t!("admin-lots-pagination"),
                    on_page_change,
                }
            }
            for (lot_id, lot_code) in selected_lot() {
                AdminProxyHistory { key: "{lot_id}", lot_id, lot_code, selected_lot, reload_generation }
            }
        }
    }
}

/// 对当前页 lot 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_lots(lots: &mut [AdminLot], column: &str, direction: SortDirection) {
    sort_rows(lots, column, direction, |left, right, column| match column {
        "code" => left.lot_number.cmp(&right.lot_number),
        "status" => left.status.cmp(&right.status),
        "start_price" => left.start_price.cmp(&right.start_price),
        "current_price" => left.current_price.cmp(&right.current_price),
        _ => Ordering::Equal,
    });
}
