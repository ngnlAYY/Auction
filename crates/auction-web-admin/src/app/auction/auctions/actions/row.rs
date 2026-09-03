//! Admin 拍卖列表行与动作入口。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminAuction, AdminAuctionCapabilities};

use super::super::super::format_admin_time;
use super::super::helpers::{
    admin_auction_status_label, admin_auction_world_label, scheduled_time_millis,
};
use super::{dialog::AdminAuctionActions, model::AdminAuctionNotice};

#[component]
pub(crate) fn AdminAuctionRow(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    clock_millis: Signal<i64>,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
) -> Element {
    let auction_id = auction.id.clone();
    let lifecycle_status = auction.status.clone();
    let current_time_millis = clock_millis();
    let open_ready = auction.status == "preview"
        && (capabilities.manual_early_open
            || scheduled_time_millis(&auction.scheduled_open_at)
                .is_some_and(|scheduled| scheduled <= current_time_millis));
    rsx! {
        tr { class: "admin-auctions__row",
            td { "data-label": t!("admin-auctions-sequence"),
                h3 { class: "admin-visually-hidden",
                    {t!("admin-auctions-row-heading", number: auction.sequence, world: auction.world_id.clone())}
                }
                "#{auction.sequence}"
            }
            td { "data-label": t!("admin-auctions-world"), {admin_auction_world_label(&auction.world_id)} }
            td { "data-label": t!("admin-auctions-status"),
                span { class: "admin-auctions__status admin-auctions__status--{auction.status}",
                    {admin_auction_status_label(&auction.status)}
                }
            }
            td { "data-label": t!("admin-auctions-open-at"), time { datetime: auction.scheduled_open_at.clone(), "{format_admin_time(&auction.scheduled_open_at)}" } }
            td {
                "data-label": t!("admin-auctions-end-at"),
                if let Some(ends_at) = auction.scheduled_ends_at.clone() {
                    time { datetime: ends_at.clone(), "{format_admin_time(&ends_at)}" }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-auctions-readiness"),
                if open_ready {
                    span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
                } else {
                    {t!("value-empty")}
                }
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
                    current_time_millis,
                    notice,
                    reload_generation,
                }
            }
        }
    }
}
