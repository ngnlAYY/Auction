//! Admin 拍卖列表行与动作入口。

use auction_web_shared::t;
use auction_web_shared::{Time, use_second_tick};
use dioxus::prelude::*;

use super::super::presentation::{
    admin_auction_status_label, admin_auction_world_label, current_time_millis,
    scheduled_time_millis,
};
use super::dialog::AdminAuctionActions;
use super::model::{AdminAuctionNotice, pending_open_millis};
use crate::api::{AdminAuction, AdminAuctionCapabilities};

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

/// 「可开拍」就绪单元格。只有等待开拍时间的预览场次挂载秒级时钟叶子；其余状态按渲染时
/// 的时间一次算定，不订阅时钟，页面没有等待中的预览行时共享 interval 随之停止。
#[component]
fn AdminAuctionReadinessCell(
    status: String,
    manual_early_open: bool,
    scheduled_open_at: String,
) -> Element {
    let current_time_millis = current_time_millis();
    let waiting_until =
        pending_open_millis(&status, manual_early_open, &scheduled_open_at, current_time_millis);
    let open_ready = status == "preview"
        && (manual_early_open
            || scheduled_time_millis(&scheduled_open_at)
                .is_some_and(|scheduled| scheduled <= current_time_millis));
    rsx! {
        td { "data-label": t!("admin-auctions-readiness"),
            if let Some(scheduled_open_millis) = waiting_until {
                AdminAuctionReadyClock { scheduled_open_millis }
            } else if open_ready {
                span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
            } else {
                {t!("value-empty")}
            }
        }
    }
}

/// 等待开拍时间的就绪标记：唯一订阅秒级节拍的就绪单元，到点后显示“可开拍”。
#[component]
fn AdminAuctionReadyClock(scheduled_open_millis: i64) -> Element {
    let second_tick = use_second_tick();
    let _ = second_tick();
    if current_time_millis() < scheduled_open_millis {
        return rsx! {
            {t!("value-empty")}
        };
    }
    rsx! {
        span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
    }
}
