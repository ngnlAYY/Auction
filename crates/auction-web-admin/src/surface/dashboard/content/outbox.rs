//! Realtime outbox 健康面板：积压、死信与最老失败年龄。

use auction_time::Timestamp;
use auction_web_shared::t;
use dioxus::prelude::*;

use super::derived::DashboardDerived;
use super::widgets::{badge, card, count_text, text};
use crate::api::AdminDashboardSnapshot;

pub(super) fn outbox_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let realtime = snapshot.realtime.data.as_ref();
    let has_realtime_consumers = derived.has_realtime_consumers;
    let pending = derived.pending;
    let failures = derived.failures;
    let outbox_state = derived.outbox_state;
    let outbox_status = derived.outbox_status.clone();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-outbox-title",
    h2 { id: "dashboard-outbox-title", {t!("dashboard-legacy-outbox-health")} }
    div { class: "admin-dashboard-cards",
        {card(
            t!("dashboard-legacy-outbox-health"),
            badge(outbox_state, outbox_status),
        )}
        {card(
            t!("dashboard-legacy-outbox-pending"),
            text(if realtime.is_some() && !has_realtime_consumers {
                t!("dashboard-legacy-outbox-status-no-consumers")
            } else {
                count_text(pending)
            }),
        )}
        {card(
            t!("dashboard-legacy-outbox-dead-letter"),
            text(count_text(failures)),
        )}
    }
    if let Some(realtime) = realtime {
        if !has_realtime_consumers {
            p { class: "admin-dashboard-status", {t!("dashboard-legacy-outbox-status-no-consumers")} }
        }
        p { class: "admin-dashboard-status",
            if realtime.failure.count == 0 {
                {t!("dashboard-legacy-outbox-no-unresolved-failures")}
            } else if let Some(seconds) = oldest_failure_age(
                &realtime.observed_at,
                realtime.failure.oldest_at.as_deref(),
            ) {
                {t!("dashboard-legacy-outbox-oldest-age", seconds: seconds)}
            } else {
                {t!("dashboard-legacy-no-data")}
            }
        }
    } else {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-outbox-metrics-unavailable")}
        }
    }
    details { class: "admin-dashboard-details",
    summary { {t!("dashboard-metrics-details")} }
    p { class: "admin-dashboard-status", {t!("dashboard-uncollected-metrics")} }
    h3 { {t!("dashboard-legacy-outbox-attempts")} }
    p { class: "admin-dashboard-status", {t!("dashboard-legacy-metric-not-collected")} }
    }
    }
    }
}

fn oldest_failure_age(observed_at: &str, oldest_at: Option<&str>) -> Option<i64> {
    let observed_at = Timestamp::parse_from_rfc3339(observed_at).ok()?;
    let oldest_at = Timestamp::parse_from_rfc3339(oldest_at?).ok()?;
    (observed_at >= oldest_at).then(|| observed_at.signed_duration_since(oldest_at).num_seconds())
}
