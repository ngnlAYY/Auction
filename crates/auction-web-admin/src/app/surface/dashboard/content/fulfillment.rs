//! 履约健康面板：积压状态、分项计数与需要人工介入的提示。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;
use crate::app::surface::Route;

use super::derived::DashboardDerived;
use super::shared::{badge, card, semantic_card, text};

pub(super) fn fulfillment_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let fulfillment = snapshot.fulfillment.data.as_ref();
    let fulfillment_state = derived.fulfillment_state;
    let fulfillment_backlog = derived.fulfillment_backlog;
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-fulfillment-title",
    h2 { id: "dashboard-fulfillment-title", {t!("dashboard-legacy-fulfillment-health")} }
    if let Some(fulfillment) = fulfillment {
        div { class: "admin-dashboard-cards",
            {semantic_card(
                "dashboard-fulfillment-state",
                t!("dashboard-legacy-fulfillment-health"),
                fulfillment_state.to_owned(),
                badge(fulfillment_state, fulfillment_status(fulfillment_state)),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-pending"),
                badge(
                    if fulfillment.pending > 0 { "warn" } else { "ok" },
                    fulfillment.pending.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-blocked"),
                badge(
                    if fulfillment.blocked > 0 { "error" } else { "ok" },
                    fulfillment.blocked.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-failed"),
                badge(
                    if fulfillment.failed > 0 { "error" } else { "ok" },
                    fulfillment.failed.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-manual-reconcile"),
                badge(
                    if fulfillment.manual_reconcile > 0 { "error" } else { "ok" },
                    fulfillment.manual_reconcile.to_string(),
                ),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-waiting-payment"),
                text(fulfillment.waiting_payment.to_string()),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-waiting-prerequisite"),
                text(fulfillment.waiting_prerequisite.to_string()),
            )}
            {card(
                t!("dashboard-legacy-fulfillment-oldest-backlog"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: fulfillment.oldest_backlog_age_seconds
                )),
            )}
        }
        if fulfillment_backlog {
            aside {
                class: "admin-dashboard-attention",
                aria_labelledby: "dashboard-fulfillment-attention-title",
                "data-state": "error",
                div {
                    class: "admin-dashboard-attention__copy",
                    role: "status",
                    aria_atomic: "true",
                    h3 {
                        id: "dashboard-fulfillment-attention-title",
                        {t!("dashboard-fulfillment-attention-title")}
                    }
                    p { {t!("dashboard-fulfillment-attention-note")} }
                }
                nav {
                    class: "admin-dashboard-attention__actions",
                    aria_label: t!("dashboard-fulfillment-attention-title"),
                    Link { to: Route::Deliveries {}, {t!("dashboard-open-deliveries")} }
                    Link { to: Route::Settlement {}, {t!("nav-settlement")} }
                }
            }
        }
    } else {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-fulfillment-status-unavailable")}
        }
    }

    }
    }
}

fn fulfillment_status(state: &str) -> String {
    match state {
        "ok" => t!("dashboard-legacy-fulfillment-status-ok"),
        "error" => t!("dashboard-legacy-fulfillment-status-degraded"),
        _ => t!("dashboard-legacy-fulfillment-status-unavailable"),
    }
}
