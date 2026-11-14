//! Realtime outbox 健康面板：积压、死信与最老失败年龄。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::derived::DashboardDerived;
use super::shared::{badge, card, text};

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
            text(if realtime.is_none() {
                t!("dashboard-legacy-no-data")
            } else if !has_realtime_consumers {
                t!("dashboard-legacy-outbox-status-no-consumers")
            } else {
                pending.to_string()
            }),
        )}
        {card(
            t!("dashboard-legacy-outbox-dead-letter"),
            text(if realtime.is_none() {
                t!("dashboard-legacy-no-data")
            } else {
                failures.to_string()
            }),
        )}
    }
    if realtime.is_none() {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-outbox-metrics-unavailable")}
        }
    } else if !has_realtime_consumers {
        p { class: "admin-dashboard-status", {t!("dashboard-legacy-outbox-status-no-consumers")} }
    } else if let Some(realtime) = realtime {
        p { class: "admin-dashboard-status",
            {t!(
                "dashboard-legacy-outbox-oldest-age",
                seconds: oldest_age(realtime.failure.oldest_at.as_deref())
            )}
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

fn oldest_age(value: Option<&str>) -> i64 {
    let Some(value) = value else {
        return 0;
    };
    #[cfg(target_arch = "wasm32")]
    {
        let parsed = js_sys::Date::parse(value);
        if !parsed.is_finite() {
            return 0;
        }
        ((js_sys::Date::now() - parsed) / 1000.0).floor().max(0.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = value;
        0
    }
}
