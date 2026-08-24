//! Admin Dashboard 运行时安全详情。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{AdminCredentialScope, AdminDashboardSnapshot};

use super::super::format_admin_time;

/// 展示后端已经提供的完整运维安全投影，避免 Dashboard 只显示摘要卡而隐藏旧版可见的健康
/// 证据。这里不执行额外命令，也不把外部凭据或数据库明细带入浏览器。
#[component]
pub(super) fn DashboardRuntimeDetails(snapshot: AdminDashboardSnapshot) -> Element {
    let health = &snapshot.health;
    let health_missing = if health.missing_services.is_empty() {
        t!("dashboard-runtime-none")
    } else {
        health.missing_services.join(", ")
    };
    let health_detail = t!(
        "dashboard-runtime-health-detail",
        status: runtime_status(&health.status),
        ready: runtime_bool(health.readiness),
        failures: health.audit_failure_count,
        missing: health_missing
    );
    let worker_detail = t!(
        "dashboard-runtime-worker-detail",
        condition: runtime_status(&snapshot.worker.condition),
        state: if snapshot.worker.state.is_empty() {
            t!("dashboard-runtime-none")
        } else {
            snapshot.worker.state.clone()
        },
        ready: runtime_bool(snapshot.worker.ready),
        version: if snapshot.worker.version.is_empty() {
            t!("dashboard-runtime-none")
        } else {
            snapshot.worker.version.clone()
        },
        pollers: snapshot.worker.mail_pollers
    );
    let realtime_detail = snapshot
        .realtime
        .data
        .as_ref()
        .map(|data| {
            let lag = data.lags.iter().map(|entry| entry.lag.max(0)).sum::<i64>();
            t!(
                "dashboard-runtime-realtime-detail",
                status: runtime_status(&snapshot.realtime.status),
                consumers: data.lags.len(),
                lag: lag,
                failures: data.failure.count,
                oldest: data.failure.oldest_at.clone().unwrap_or_else(|| t!("dashboard-runtime-none")),
                latest_code: if data.failure.latest_code.is_empty() {
                    t!("dashboard-runtime-none")
                } else {
                    data.failure.latest_code.clone()
                }
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.realtime.error_code.as_deref()));
    let credential_detail = snapshot
        .credentials
        .data
        .as_ref()
        .map(|data| {
            t!(
                "dashboard-runtime-credentials-detail",
                status: runtime_status(&snapshot.credentials.status),
                active: data.active_count,
                disabled: data.disabled_count,
                invalid: data.invalid_count,
                scopes: data.scopes.len()
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.credentials.error_code.as_deref()));
    let delivery_detail = snapshot
        .delivery
        .data
        .as_ref()
        .map(|data| {
            let water = &data.water;
            t!(
                "dashboard-runtime-delivery-detail",
                status: runtime_status(&snapshot.delivery.status),
                pending: water.pending,
                failed: water.failed,
                manual: water.manual_reconcile,
                oldest_pending: water.oldest_pending_age_seconds,
                oldest_manual: water.oldest_manual_reconcile_age_seconds
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.delivery.error_code.as_deref()));
    let fulfillment_detail = snapshot
        .fulfillment
        .data
        .as_ref()
        .map(|data| {
            t!(
                "dashboard-runtime-fulfillment-detail",
                status: runtime_status(&snapshot.fulfillment.status),
                pending: data.pending,
                blocked: data.blocked,
                failed: data.failed,
                manual: data.manual_reconcile,
                oldest: data.oldest_backlog_age_seconds
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.fulfillment.error_code.as_deref()));
    let recovery_detail = snapshot
        .recovery
        .data
        .as_ref()
        .map(|data| {
            t!(
                "dashboard-runtime-recovery-detail",
                status: runtime_status(&snapshot.recovery.status),
                overdue: data.payment_overdue_count,
                relist: data.relist_pending_count,
                failed: data.failed_action_count,
                manual: data.manual_reconcile_count,
                oldest: data.oldest_overdue_age_seconds,
                worlds: data.worlds.len()
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.recovery.error_code.as_deref()));
    let alerting_detail = snapshot
        .alerting
        .data
        .as_ref()
        .map(|data| {
            t!(
                "dashboard-runtime-alerting-detail",
                status: runtime_status(&snapshot.alerting.status),
                enabled: runtime_bool(data.enabled),
                active: data.active_signal_count,
                due: data.due_signal_count,
                claimed: data.claimed_batch_count,
                succeeded: data.succeeded_batch_count,
                failed: data.failed_batch_count,
                manual: data.manual_reconcile_count,
                last_observed: data
                    .last_observed_at
                    .as_deref()
                    .map(format_admin_time)
                    .unwrap_or_else(|| t!("dashboard-runtime-none")),
                last_success: data
                    .last_success_at
                    .as_deref()
                    .map(format_admin_time)
                    .unwrap_or_else(|| t!("dashboard-runtime-none")),
                last_failure: data
                    .last_failure_at
                    .as_deref()
                    .map(format_admin_time)
                    .unwrap_or_else(|| t!("dashboard-runtime-none")),
                last_error: if data.last_error_code.is_empty() {
                    t!("dashboard-runtime-none")
                } else {
                    data.last_error_code.clone()
                }
            )
        })
        .unwrap_or_else(|| section_unavailable(snapshot.alerting.error_code.as_deref()));

    rsx! {
        section { class: "admin-dashboard__runtime-details", aria_labelledby: "dashboard-runtime-title",
            header { class: "admin-dashboard__runtime-header",
                div {
                    p { class: "admin-dashboard__card-kicker", {t!("dashboard-runtime-kicker")} }
                    h2 { id: "dashboard-runtime-title", {t!("dashboard-runtime-title")} }
                }
                span { class: "admin-dashboard__readonly", {t!("dashboard-readonly")} }
            }
            div { class: "admin-dashboard__detail-grid",
                DashboardDetailCard {
                    title: t!("dashboard-runtime-health-title"),
                    detail: health_detail,
                    tone: runtime_tone(&health.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-worker-title"),
                    detail: worker_detail,
                    tone: runtime_tone(&snapshot.worker.condition),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-realtime-title"),
                    detail: realtime_detail,
                    tone: runtime_tone(&snapshot.realtime.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-credentials-title"),
                    detail: credential_detail,
                    tone: runtime_tone(&snapshot.credentials.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-delivery-title"),
                    detail: delivery_detail,
                    tone: runtime_tone(&snapshot.delivery.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-fulfillment-title"),
                    detail: fulfillment_detail,
                    tone: runtime_tone(&snapshot.fulfillment.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-recovery-title"),
                    detail: recovery_detail,
                    tone: runtime_tone(&snapshot.recovery.status),
                }
                DashboardDetailCard {
                    title: t!("dashboard-runtime-alerting-title"),
                    detail: alerting_detail,
                    tone: runtime_tone(&snapshot.alerting.status),
                }
            }

            if let Some(process_data) = snapshot.process.data.as_ref() {
                section { class: "admin-dashboard__runtime-list", aria_labelledby: "dashboard-processes-title",
                    h3 { id: "dashboard-processes-title", {t!("dashboard-runtime-processes-title")} }
                    div { class: "admin-dashboard__detail-grid",
                        for process in &process_data.processes {
                            DashboardDetailCard {
                                title: process.process_role.clone(),
                                detail: t!(
                                    "dashboard-runtime-process-detail",
                                    service: process.service_name.clone(),
                                    status: runtime_status(&process.status),
                                    ready: runtime_bool(process.readiness),
                                    version: process.version.clone(),
                                    tracing: runtime_status(&process.tracing.status),
                                    dropped: process.tracing.dropped_span_count,
                                    export_failures: process.tracing.export_failure_count,
                                    audit_failures: process.audit_failure_count
                                ),
                                tone: runtime_tone(&process.status),
                            }
                        }
                    }
                }
            }

            if let Some(recovery_data) = snapshot.recovery.data.as_ref() {
                section { class: "admin-dashboard__runtime-list", aria_labelledby: "dashboard-recovery-worlds-title",
                    h3 { id: "dashboard-recovery-worlds-title", {t!("dashboard-runtime-recovery-worlds-title")} }
                    div { class: "admin-dashboard__detail-grid",
                        for world in &recovery_data.worlds {
                            DashboardDetailCard {
                                title: world.world.clone(),
                                detail: t!(
                                    "dashboard-runtime-recovery-world-detail",
                                    balance: world.balance_credits,
                                    balance_status: runtime_status(&world.balance_status),
                                    evidence_status: runtime_status(&world.evidence_status),
                                    evidence_age: world.evidence_age_seconds.unwrap_or(0),
                                    updated: format_admin_time(&world.updated_at)
                                ),
                                tone: runtime_tone(&world.balance_status),
                            }
                        }
                    }
                }
            }

            if let Some(credentials_data) = snapshot.credentials.data.as_ref() {
                if !credentials_data.scopes.is_empty() {
                    section { class: "admin-dashboard__runtime-list", aria_labelledby: "dashboard-credential-scopes-title",
                        h3 { id: "dashboard-credential-scopes-title", {t!("dashboard-runtime-credential-scopes-title")} }
                        div { class: "admin-dashboard__detail-grid",
                            for scope in &credentials_data.scopes {
                                DashboardDetailCard {
                                    title: format!("{} · {}", scope.world, scope.kind),
                                    detail: t!(
                                        "dashboard-runtime-credential-scope-detail",
                                        status: runtime_status(credential_scope_status(scope)),
                                        active: scope.active_count,
                                        disabled: scope.disabled_count,
                                        invalid: scope.invalid_count,
                                        updated: scope
                                            .updated_at
                                            .as_deref()
                                            .map(format_admin_time)
                                            .unwrap_or_else(|| t!("dashboard-runtime-none")),
                                        restart: runtime_bool(scope.restart_required.unwrap_or(false))
                                    ),
                                    tone: credential_scope_tone(scope),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DashboardDetailCard(title: String, detail: String, tone: String) -> Element {
    rsx! {
        article { class: "admin-dashboard__detail-card admin-dashboard__detail-card--{tone}",
            h3 { "{title}" }
            p { "{detail}" }
        }
    }
}

fn runtime_bool(value: bool) -> String {
    if value { t!("dashboard-runtime-yes") } else { t!("dashboard-runtime-no") }
}

fn runtime_status(value: &str) -> String {
    match value {
        "ok" | "ready" | "available" | "fresh" => t!("dashboard-runtime-status-good"),
        "degraded" | "low" | "stale" | "starting" => t!("dashboard-runtime-status-warning"),
        "disabled" => t!("dashboard-runtime-status-disabled"),
        "unknown" | "unavailable" | "stopping" => t!("dashboard-runtime-status-unavailable"),
        _ => value.to_owned(),
    }
}

fn runtime_tone(value: &str) -> String {
    match value {
        "ok" | "ready" | "available" | "fresh" => "positive".to_owned(),
        "degraded" | "low" | "stale" | "starting" => "warning".to_owned(),
        "unknown" | "unavailable" | "stopping" => "danger".to_owned(),
        _ => "neutral".to_owned(),
    }
}

fn credential_scope_status(scope: &AdminCredentialScope) -> &'static str {
    if scope.invalid_count > 0 {
        "unavailable"
    } else if scope.active_count > 0 {
        "ok"
    } else {
        "degraded"
    }
}

fn credential_scope_tone(scope: &AdminCredentialScope) -> String {
    match credential_scope_status(scope) {
        "ok" => "positive".to_owned(),
        "degraded" => "warning".to_owned(),
        _ => "danger".to_owned(),
    }
}

pub(super) fn section_unavailable(error_code: Option<&str>) -> String {
    match error_code {
        Some("no_active_credentials") => t!("dashboard-runtime-no-active-credentials"),
        Some("invalid_credentials") => t!("dashboard-runtime-invalid-credentials"),
        Some("credentials_restart_required") => {
            t!("dashboard-runtime-credentials-restart-required")
        }
        Some("credentials_restart_unknown") => t!("dashboard-runtime-credentials-restart-unknown"),
        Some("unresolved_failure") => t!("dashboard-runtime-unresolved-failure"),
        Some("consumer_lag") => t!("dashboard-runtime-consumer-lag"),
        Some("source_unavailable") | Some("no_pollers") | None => {
            t!("dashboard-runtime-unavailable")
        }
        Some(_) => t!("dashboard-runtime-degraded"),
    }
}
