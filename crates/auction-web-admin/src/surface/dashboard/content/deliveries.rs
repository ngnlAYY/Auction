//! 投递水位面板：按状态展示待处理、失败与人工核对数量。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::shared::{badge, card, text};

pub(super) fn deliveries_panel(snapshot: &AdminDashboardSnapshot) -> Element {
    let delivery = snapshot.delivery.data.as_ref();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-deliveries-title",
    h2 { id: "dashboard-deliveries-title", {t!("dashboard-legacy-deliveries-by-status")} }
    if let Some(delivery) = delivery {
        div { class: "admin-dashboard-cards",
            {card(
                t!("dashboard-legacy-delivery-pending"),
                text(delivery.water.pending.to_string()),
            )}
            {card(
                t!("dashboard-legacy-delivery-failed"),
                badge(
                    if delivery.water.failed > 0 { "error" } else { "ok" },
                    delivery.water.failed.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-delivery-manual-reconcile"),
                badge(
                    if delivery.water.manual_reconcile > 0 { "error" } else { "ok" },
                    delivery.water.manual_reconcile.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-delivery-oldest-pending"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: delivery.water.oldest_pending_age_seconds
                )),
            )}
            {card(
                t!("dashboard-legacy-delivery-oldest-manual-reconcile"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: delivery.water.oldest_manual_reconcile_age_seconds
                )),
            )}
        }
    } else {
        p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
    }

    }
    }
}
