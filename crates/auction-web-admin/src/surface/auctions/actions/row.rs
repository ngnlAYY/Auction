//! Admin 拍卖列表行与动作入口。

use auction_web_shared::t;
use auction_web_shared::{Time, use_second_tick};
use dioxus::prelude::*;

use super::super::presentation::{
    admin_auction_status_label, current_time_millis, scheduled_time_millis,
};
use super::dialog::AdminAuctionActions;
use super::model::{AdminAuctionNotice, pending_open_millis};
use crate::api::{AdminAuction, AdminAuctionCapabilities};
use crate::surface::admin_world_label;

#[component]
pub(crate) fn AdminAuctionRow(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    edits_ready: bool,
    notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    rsx! {
        tr { class: "admin-auctions__row",
            td { "data-label": t!("admin-auctions-sequence"), "#{auction.sequence}" }
            td { "data-label": t!("admin-auctions-world"), {admin_world_label(&auction.world_id)} }
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
                    auction: auction.clone(),
                    capabilities,
                    edits_ready,
                    notice,
                }
            }
        }
    }
}

/// 「可开拍」就绪单元格。只有渲染时仍在等待开拍时间的预览场次挂载秒级时钟叶子；其余状态
/// 按渲染时的时间一次算定，不订阅时钟。到点后叶子仍挂载并继续订阅，直到本行因列表刷新等
/// 原因重新渲染才卸载；页面上不再有挂载中的时钟叶子时，共享 interval 才停止。
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
