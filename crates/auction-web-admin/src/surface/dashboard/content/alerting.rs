//! 告警投递面板：信号/批次计数与时间戳明细。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::shared::{badge, card, text};

pub(super) fn alerting_panel(snapshot: &AdminDashboardSnapshot) -> Element {
    let alerting = snapshot.alerting.data.as_ref();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-alerting-title",
    h2 { id: "dashboard-alerting-title", {t!("dashboard-legacy-alerting-health")} }
    if let Some(alerting) = alerting {
        div { class: "admin-dashboard-cards",
            {card(
                t!("dashboard-legacy-alerting-health"),
                badge(
                    if !alerting.enabled {
                        "neutral"
                    } else if snapshot.alerting.status == "ok" {
                        "ok"
                    } else {
                        "error"
                    },
                    if !alerting.enabled {
                        t!("dashboard-legacy-alerting-status-disabled")
                    } else if snapshot.alerting.status == "ok" {
                        t!("dashboard-legacy-alerting-status-ok")
                    } else {
                        t!("dashboard-legacy-alerting-status-degraded")
                    },
                ),
            )}
            {card(
                t!("dashboard-legacy-alerting-active"),
                text(alerting.active_signal_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-due"),
                text(alerting.due_signal_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-claimed"),
                text(alerting.claimed_batch_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-succeeded"),
                text(alerting.succeeded_batch_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-failed"),
                text(alerting.failed_batch_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-manual-reconcile"),
                text(alerting.manual_reconcile_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-webhook-pending"),
                text(alerting.webhook_pending_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-webhook-sending"),
                text(alerting.webhook_sending_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-webhook-failed"),
                text(alerting.webhook_failed_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-alerting-webhook-manual"),
                text(alerting.webhook_manual_reconcile_count.to_string()),
            )}
        }
        details { class: "admin-dashboard-details",
            summary { {t!("dashboard-metrics-details")} }
            div { class: "admin-dashboard-cards admin-dashboard-cards--timestamps",
            {card(
                t!("dashboard-legacy-alerting-last-observed"),
                text(alerting.last_observed_at.clone().unwrap_or_else(|| {
                    t!("dashboard-legacy-not-available")
                })),
            )}
            {card(
                t!("dashboard-legacy-alerting-last-success"),
                text(alerting.last_success_at.clone().unwrap_or_else(|| {
                    t!("dashboard-legacy-not-available")
                })),
            )}
            {card(
                t!("dashboard-legacy-alerting-last-failure"),
                text(alerting.last_failure_at.clone().unwrap_or_else(|| {
                    t!("dashboard-legacy-not-available")
                })),
            )}
            {card(
                t!("dashboard-legacy-alerting-last-error"),
                text(if alerting.last_error_code.is_empty() {
                    t!("dashboard-legacy-not-available")
                } else {
                    alerting.last_error_code.clone()
                }),
            )}
            }
        }
    } else {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-alerting-status-unavailable")}
        }
    }
    }
    }
}
