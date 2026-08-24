//! Admin Dashboard 摘要指标。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminAlertingSection, AdminAuctionPage, AdminCredentialData, AdminCredentialSection,
    AdminMailData, AdminMailIntakePage, AdminMailSection, AdminRealtimeData, DeliveryPage,
};

#[derive(Clone, PartialEq)]
pub(super) struct DashboardMetric {
    value: String,
    state: String,
    detail: String,
    tone: &'static str,
}

#[component]
pub(super) fn MetricCard(label: String, metric: DashboardMetric) -> Element {
    rsx! {
        article { class: "admin-metric-card admin-metric-card--{metric.tone}",
            div { class: "admin-metric-card__topline",
                span { class: "admin-metric-card__label", "{label}" }
                span { class: "admin-status-pill admin-status-pill--{metric.tone}", "{metric.state}" }
            }
            strong { class: "admin-metric-card__value", "{metric.value}" }
            p { class: "admin-metric-card__detail", "{metric.detail}" }
        }
    }
}

#[component]
pub(super) fn DashboardStatusRow(label: String, metric: DashboardMetric) -> Element {
    rsx! {
        div { class: "admin-dashboard__status-row",
            div { class: "admin-dashboard__status-label",
                span { class: "admin-dashboard__status-dot admin-dashboard__status-dot--{metric.tone}", aria_hidden: "true" }
                span { "{label}" }
            }
            strong { "{metric.state}" }
            small { "{metric.detail}" }
        }
    }
}

pub(super) fn auction_metric(state: &Option<Result<AdminAuctionPage, String>>) -> DashboardMetric {
    match state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(page)) => {
            let active = page
                .auctions
                .iter()
                .filter(|auction| {
                    matches!(auction.status.as_str(), "preview" | "live" | "final" | "settling")
                })
                .count();
            DashboardMetric {
                value: page.pagination.total.to_string(),
                state: t!("dashboard-metric-connected"),
                detail: t!("dashboard-auctions-detail", active: active, total: page.pagination.total),
                tone: if active == 0 { "neutral" } else { "positive" },
            }
        }
    }
}

pub(super) fn delivery_metric(state: &Option<Result<DeliveryPage, String>>) -> DashboardMetric {
    match state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(page)) => {
            let attention = page
                .deliveries
                .iter()
                .filter(|delivery| {
                    matches!(delivery.status.as_str(), "failed" | "manual_reconcile")
                })
                .count();
            DashboardMetric {
                value: page.total.to_string(),
                state: if attention == 0 {
                    t!("dashboard-metric-connected")
                } else {
                    t!("dashboard-metric-attention")
                },
                detail: t!("dashboard-deliveries-detail", attention: attention, total: page.total),
                tone: if attention == 0 { "positive" } else { "warning" },
            }
        }
    }
}

pub(super) fn realtime_metric(
    state: &Option<Result<AdminRealtimeData, String>>,
) -> DashboardMetric {
    match state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(data)) => {
            let total_lag = data.lags.iter().map(|value| value.lag.max(0)).sum::<i64>();
            let tone = if data.failure.count > 0 {
                "danger"
            } else if total_lag > 0 {
                "warning"
            } else {
                "positive"
            };
            DashboardMetric {
                value: data.failure.count.to_string(),
                state: if tone == "positive" {
                    t!("dashboard-metric-connected")
                } else {
                    t!("dashboard-metric-attention")
                },
                detail: t!(
                    "dashboard-realtime-detail",
                    lag: total_lag,
                    failures: data.failure.count
                ),
                tone,
            }
        }
    }
}

pub(super) fn mail_metric(
    dashboard_state: &Option<Result<AdminMailSection, String>>,
    intake_state: &Option<Result<AdminMailIntakePage, String>>,
) -> DashboardMetric {
    match dashboard_state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(section)) => {
            let Some(AdminMailData { pollers }) = section.data.as_ref() else {
                return unavailable_metric();
            };
            if pollers.is_empty() {
                return unavailable_metric();
            }
            let failures =
                pollers.iter().map(|polling| polling.consecutive_failures.max(0)).sum::<i32>();
            let paused = pollers.iter().filter(|polling| polling.paused).count();
            let intakes =
                intake_state.as_ref().and_then(|result| result.as_ref().ok()).map_or_else(
                    || t!("dashboard-metric-unavailable"),
                    |page| page.pagination.total.to_string(),
                );
            let needs_attention = paused > 0 || failures > 0;
            DashboardMetric {
                value: pollers.len().to_string(),
                state: if needs_attention {
                    t!("dashboard-metric-attention")
                } else {
                    t!("dashboard-metric-connected")
                },
                detail: t!(
                    "dashboard-mail-detail",
                    failures: failures,
                    intakes: intakes,
                    pollers: pollers.len()
                ),
                tone: if paused > 0 {
                    "warning"
                } else if failures > 0 {
                    "danger"
                } else {
                    "positive"
                },
            }
        }
    }
}

pub(super) fn alerting_metric(
    state: &Option<Result<AdminAlertingSection, String>>,
) -> DashboardMetric {
    match state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(section)) => {
            let Some(data) = section.data.as_ref() else {
                return unavailable_metric();
            };
            if !data.enabled {
                return unavailable_metric();
            }
            let attention = data
                .due_signal_count
                .saturating_add(data.claimed_batch_count)
                .saturating_add(data.failed_batch_count)
                .saturating_add(data.manual_reconcile_count);
            let tone = if data.failed_batch_count > 0 || data.manual_reconcile_count > 0 {
                "danger"
            } else if attention > 0 || section.status == "degraded" {
                "warning"
            } else {
                "positive"
            };
            DashboardMetric {
                value: data.active_signal_count.to_string(),
                state: if tone == "positive" {
                    t!("dashboard-metric-connected")
                } else {
                    t!("dashboard-metric-attention")
                },
                detail: t!(
                    "dashboard-alerting-detail",
                    due: data.due_signal_count,
                    claimed: data.claimed_batch_count,
                    failed: data.failed_batch_count,
                    manual: data.manual_reconcile_count
                ),
                tone,
            }
        }
    }
}

pub(super) fn credential_metric(
    state: &Option<Result<AdminCredentialSection, String>>,
) -> DashboardMetric {
    match state {
        None => loading_metric(),
        Some(Err(_)) => unavailable_metric(),
        Some(Ok(section)) => match section.data.as_ref() {
            None => unavailable_metric(),
            Some(AdminCredentialData { active_count, disabled_count, invalid_count, .. }) => {
                let tone = if *invalid_count > 0 {
                    "danger"
                } else if *active_count == 0 || section.status != "ok" {
                    "warning"
                } else {
                    "positive"
                };
                DashboardMetric {
                    value: active_count.to_string(),
                    state: if tone == "positive" {
                        t!("dashboard-metric-connected")
                    } else {
                        t!("dashboard-metric-attention")
                    },
                    detail: t!(
                        "dashboard-credentials-detail",
                        active: active_count,
                        disabled: disabled_count,
                        invalid: invalid_count
                    ),
                    tone,
                }
            }
        },
    }
}

fn loading_metric() -> DashboardMetric {
    DashboardMetric {
        value: t!("value-empty"),
        state: t!("dashboard-metric-loading"),
        detail: t!("dashboard-metric-loading-detail"),
        tone: "neutral",
    }
}

fn unavailable_metric() -> DashboardMetric {
    DashboardMetric {
        value: t!("value-empty"),
        state: t!("dashboard-metric-unavailable"),
        detail: t!("dashboard-metric-unavailable-detail"),
        tone: "danger",
    }
}
