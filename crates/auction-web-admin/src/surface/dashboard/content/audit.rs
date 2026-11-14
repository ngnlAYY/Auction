//! 审计健康面板：旧版 audit 指标与缺失服务的可用性说明。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::derived::DashboardDerived;
use super::shared::{badge, card, text};

pub(super) fn audit_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let audit_failures = derived.audit_failures;
    let audit_metrics_unavailable = derived.audit_metrics_unavailable;
    let audit_state = derived.audit_state;
    let audit_status = derived.audit_status.clone();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-audit-title",
    h2 { id: "dashboard-audit-title", {t!("dashboard-legacy-audit-health")} }
    div { class: "admin-dashboard-cards",
        {card(
            t!("dashboard-legacy-audit-health"),
            badge(audit_state, audit_status),
        )}
        {card(
            t!("dashboard-legacy-audit-failures-total"),
            text(audit_failures.to_string()),
        )}
    }
    p { class: "admin-dashboard-status",
        {t!(
            "dashboard-legacy-audit-metrics-line",
            api: availability(snapshot, "public"),
            worker: availability(snapshot, "worker")
        )}
    }
    details { class: "admin-dashboard-details",
    summary { {t!("dashboard-metrics-details")} }
    p { class: "admin-dashboard-status", {t!("dashboard-legacy-audit-scope")} }
    if audit_failures == 0 && !audit_metrics_unavailable {
        p { class: "admin-dashboard-status", {t!("dashboard-legacy-audit-no-failures")} }
    }
    }
    }
    }
}

fn availability(snapshot: &AdminDashboardSnapshot, service: &str) -> String {
    if snapshot.health.missing_services.iter().any(|missing| missing == service) {
        t!("dashboard-legacy-audit-metrics-unavailable")
    } else {
        t!("dashboard-legacy-audit-metrics-available")
    }
}
