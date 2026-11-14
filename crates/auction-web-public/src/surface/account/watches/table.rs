//! 当前场次完整标记结果的本地分页和语义表格；不把目录总数冒充标记数。

use std::rc::Rc;

use auction_web_shared::{TruncatedText, t};
use dioxus::prelude::*;

use super::load::WatchedAuction;
use crate::surface::{
    LotDetailLookupButton, PublicPager, auction_status_label, lot_status_label, world_label,
};

const PAGE_SIZE: i64 = 10;

/// 按完整交集的真实总数分页，场次 keyed 宿主保证新场次从第一页开始。
#[component]
pub(super) fn WatchedTable(watched: Rc<WatchedAuction>) -> Element {
    let mut requested_page = use_signal(|| 1_i64);
    let total = watched.rows.len() as i64;
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = requested_page().clamp(1, pages);
    let offset = ((page - 1) * PAGE_SIZE) as usize;
    let auction = &watched.auction;
    let round =
        t!("account-watches-round", world: world_label(&auction.world), number: auction.number);
    let title = t!("account-watches-heading", round: round.clone(), status: auction_status_label(&auction.status));
    rsx! {
        h3 { "{title}" }
        if total == 0 {
            p { class: "public-account-empty", {t!("account-watches-empty")} }
        } else {
            p { class: "public-account-hint", {t!("account-watches-total", count: total)} }
            div { class: "public-account-table-wrap",
                table { class: "public-account-table",
                    aria_label: "{title}",
                    auction_web_shared::TableColumns { layout: "public-account-watches", widths: vec![13, 42, 22, 15, 8] }
                    thead { tr {
                        th { scope: "col", {t!("account-lot")} }
                        th { scope: "col", {t!("account-watches-equipment")} }
                        th { scope: "col", {t!("account-watches-auction")} }
                        th { scope: "col", {t!("account-status")} }
                        th { scope: "col", class: "public-lot-view-cell", {t!("catalog-actions")} }
                    } }
                    tbody {
                        for row in watched.rows.iter().skip(offset).take(PAGE_SIZE as usize) {
                            tr { key: "{row.lot_id}",
                                // 编号与名称不再跳转；拍品详情统一经操作栏的查看按钮在页内抽屉打开。
                                td { class: "public-account-code", "data-label": t!("account-lot"), "{row.code}" }
                                td { "data-label": t!("account-watches-equipment"),
                                    TruncatedText { value: row.name.clone(), size: "wide", portal: true }
                                }
                                td { "data-label": t!("account-watches-auction"),
                                    TruncatedText { value: round.clone(), size: "compact", portal: true }
                                }
                                td { "data-label": t!("account-status"),
                                    TruncatedText { value: lot_status_label(&row.status), size: "compact", portal: true }
                                }
                                td { class: "public-lot-view-cell", "data-label": t!("catalog-actions"),
                                    LotDetailLookupButton {
                                        auction_id: auction.id.clone(),
                                        lot_id: row.lot_id.clone(),
                                        lot_code: row.code.clone(),
                                        world: Some(auction.world.clone()),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // 始终把原页码交给纠偏宿主；空或单页只隐藏控件，不能留下会在总数恢复后跳回的旧页码。
        PublicPager {
            label: title,
            page: requested_page(),
            total: Some(total),
            page_size: PAGE_SIZE,
            hide_when_single: true,
            show_size_picker: false,
            max_offset: None,
            jump_label: Some(t!("catalog-page-input")),
            on_page_change: move |page| requested_page.set(page),
            on_page_clamp: move |page| requested_page.set(page),
        }
    }
}
