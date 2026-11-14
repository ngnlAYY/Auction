//! 进程追踪健康面板：逐进程展示 tracing 状态与导出计数。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::widgets::badge;
use crate::api::AdminDashboardSnapshot;

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
                // 列表是全部未过期心跳，重启交接时同一角色会短暂并存新旧实例；key 取心跳主键，
                // 角色不能保证唯一。
                div { class: "admin-dashboard-card", key: "{item.service_name}/{item.instance_id}",
                    div { class: "admin-dashboard-value",
                        span { class: "admin-dashboard-metric-stack",
                            {badge(tracing_tone(&item.tracing.status), tracing_status(&item.tracing.status))}
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
                        {t!("dashboard-legacy-tracing-process", role: item.process_role.clone())}
                    }
                }
            }
        }
    }

    }
    }
}

fn tracing_tone(status: &str) -> &'static str {
    match status {
        "disabled" => "neutral",
        "ok" => "ok",
        _ => "error",
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
