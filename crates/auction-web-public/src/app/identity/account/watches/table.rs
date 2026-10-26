//! 当前场次完整标记结果的本地分页和语义表格；不把目录总数冒充标记数。

use auction_web_shared::{TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use crate::app::surface::{
    auction_status_label, lot_detail_world_href, lot_status_label, world_label,
};

use super::super::AccountPager;
use super::load::WatchedAuction;

const PAGE_SIZE: i64 = 10;

/// 按完整交集的真实总数分页，场次 keyed 宿主保证新场次从第一页开始。
#[component]
pub(super) fn WatchedTable(watched: WatchedAuction) -> Element {
    let mut requested_page = use_signal(|| 1_i64);
    let total = watched.rows.len() as i64;
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = requested_page().clamp(1, pages);
    let offset = ((page - 1) * PAGE_SIZE) as usize;
    let auction = watched.auction;
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
                    caption { class: "public-visually-hidden", "{title}" }
                    auction_web_shared::TableColumns { layout: "public-account-watches", widths: vec![14, 46, 24, 16] }
                    thead { tr {
                        th { scope: "col", {t!("account-lot")} }
                        th { scope: "col", {t!("account-watches-equipment")} }
                        th { scope: "col", {t!("account-watches-auction")} }
                        th { scope: "col", {t!("account-status")} }
                    } }
                    tbody {
                        for row in watched.rows.into_iter().skip(offset).take(PAGE_SIZE as usize) {
                            tr { key: "{row.lot_id}",
                                td { class: "public-account-code", "data-label": t!("account-lot"),
                                    Link { to: lot_detail_world_href(&row.lot_id, &auction.world), "{row.code}" }
                                }
                                td { "data-label": t!("account-watches-equipment"),
                                    Link { to: lot_detail_world_href(&row.lot_id, &auction.world),
                                        TruncatedText { value: row.name, size: "wide", focusable: false, portal: true }
                                    }
                                }
                                td { "data-label": t!("account-watches-auction"),
                                    TruncatedText { value: round.clone(), size: "compact", portal: true }
                                }
                                td { "data-label": t!("account-status"),
                                    TruncatedText { value: lot_status_label(&row.status), size: "compact", portal: true }
                                }
                            }
                        }
                    }
                }
            }
            AccountPager {
                label: title,
                page,
                total,
                page_size: PAGE_SIZE,
                on_page_change: move |page| requested_page.set(page),
                on_page_clamp: move |page| requested_page.set(page),
            }
        }
    }
}
