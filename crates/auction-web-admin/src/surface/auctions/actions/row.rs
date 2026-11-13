//! Admin 拍卖列表行与动作入口。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{Time, use_second_tick};

use crate::api::{AdminAuction, AdminAuctionCapabilities};

use super::super::helpers::{
    admin_auction_status_label, admin_auction_world_label, current_time_millis,
    scheduled_time_millis,
};
use super::{dialog::AdminAuctionActions, model::AdminAuctionNotice};

#[component]
pub(crate) fn AdminAuctionRow(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let auction_id = auction.id.clone();
    let lifecycle_status = auction.status.clone();
    rsx! {
        tr { class: "admin-auctions__row",
            td { "data-label": t!("admin-auctions-sequence"), "#{auction.sequence}" }
            td { "data-label": t!("admin-auctions-world"), {admin_auction_world_label(&auction.world_id)} }
            td { "data-label": t!("admin-auctions-status"),
                span { class: "admin-auctions__status admin-auctions__status--{auction.status}",
                    {admin_auction_status_label(&auction.status)}
                }
            }
            td { "data-label": t!("admin-auctions-open-at"), Time { value: auction.scheduled_open_at.clone() } }
            td {
                "data-label": t!("admin-auctions-end-at"),
                if let Some(ends_at) = auction.scheduled_ends_at.clone() {
                    Time { value: ends_at }
                } else {
                    {t!("value-empty")}
                }
            }
            AdminAuctionReadinessCell {
                status: auction.status.clone(),
                manual_early_open: capabilities.manual_early_open,
                scheduled_open_at: auction.scheduled_open_at.clone(),
            }
            td { "data-label": t!("admin-auctions-actions"),
                AdminAuctionActions {
                    auction_id,
                    auction_sequence: auction.sequence,
                    status: lifecycle_status,
                    expected_state_version: auction.state_version,
                    capabilities,
                    preview_scheduled_at: auction.preview_scheduled_at.clone(),
                    scheduled_open_at: auction.scheduled_open_at.clone(),
                    scheduled_end_at: auction.scheduled_ends_at.clone(),
                    notice,
                }
            }
        }
    }
}

/// 「可开拍」就绪单元格。秒级节拍只在本叶子组件内订阅：整行与整表不随秒针重渲染，
/// 只有依赖当前时间的就绪判定每秒重算一次。
#[component]
fn AdminAuctionReadinessCell(
    status: String,
    manual_early_open: bool,
    scheduled_open_at: String,
) -> Element {
    let second_tick = use_second_tick();
    let _ = second_tick();
    let current_time_millis = current_time_millis();
    let open_ready = status == "preview"
        && (manual_early_open
            || scheduled_time_millis(&scheduled_open_at)
                .is_some_and(|scheduled| scheduled <= current_time_millis));
    rsx! {
        td { "data-label": t!("admin-auctions-readiness"),
            if open_ready {
                span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
            } else {
                {t!("value-empty")}
            }
        }
    }
}
