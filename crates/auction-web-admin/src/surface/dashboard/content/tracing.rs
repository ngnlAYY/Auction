//! 进程追踪健康面板：逐进程展示 tracing 状态与导出计数。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::shared::badge;

pub(super) fn tracing_panel(snapshot: &AdminDashboardSnapshot) -> Element {
    let process = snapshot.process.data.as_ref();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-tracing-title",
    h2 { id: "dashboard-tracing-title", {t!("dashboard-legacy-tracing-health")} }
    if process.is_none_or(|data| data.processes.is_empty()) {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-tracing-status-unavailable")}
        }
    } else if let Some(process) = process {
        div { class: "admin-dashboard-cards",
            for item in process.processes.iter() {
                {
                    let tone = match item.tracing.status.as_str() {
                        "disabled" => "neutral",
                        "ok" => "ok",
                        _ => "error",
                    };
                    let role = item.process_role.clone();
                    rsx! {
                        div { class: "admin-dashboard-card", key: "{role}",
                            div { class: "admin-dashboard-value",
                                span { class: "admin-dashboard-metric-stack",
                                    {badge(tone, tracing_status(&item.tracing.status))}
                                    span { class: "admin-dashboard-label",
                                        {t!(
                                            "dashboard-legacy-tracing-counts",
                                            dropped: item.tracing.dropped_span_count,
                                            failures: item.tracing.export_failure_count
                                        )}
                                    }
                                }
                            }
                            div { class: "admin-dashboard-label",
                                {t!("dashboard-legacy-tracing-process", role: role)}
                            }
                        }
                    }
                }
            }
        }
    }

    }
    }
}

fn tracing_status(status: &str) -> String {
    match status {
        "disabled" => t!("dashboard-legacy-tracing-status-disabled"),
        "ok" => t!("dashboard-legacy-tracing-status-ok"),
        "degraded" => t!("dashboard-legacy-tracing-status-degraded"),
        _ => t!("dashboard-legacy-tracing-status-unavailable"),
    }
}
