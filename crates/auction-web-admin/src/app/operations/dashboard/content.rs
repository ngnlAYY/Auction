//! 旧版 Admin Dashboard 的指标顺序与展示闭集。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::super::Route;

#[component]
pub(super) fn DashboardContent(snapshot: AdminDashboardSnapshot) -> Element {
    let process = snapshot.process.data.as_ref();
    let mail = snapshot.mail.data.as_ref();
    let poll_paused = mail.is_some_and(|data| data.pollers.iter().any(|poller| poller.paused));
    let realtime = snapshot.realtime.data.as_ref();
    let has_realtime_consumers = realtime.is_some_and(|data| !data.lags.is_empty());
    let pending = realtime.map_or(0, |data| {
        data.lags.iter().fold(0_i64, |total, item| total.saturating_add(item.lag))
    });
    let failures = realtime.map_or(0, |data| data.failure.count);
    let outbox_state = if realtime.is_none() {
        "neutral"
    } else if failures > 0 {
        "error"
    } else if !has_realtime_consumers {
        "neutral"
    } else if pending > 0 {
        "warn"
    } else {
        "ok"
    };
    let outbox_status = if realtime.is_none() {
        t!("dashboard-legacy-outbox-status-unavailable")
    } else if failures > 0 {
        t!("dashboard-legacy-outbox-status-dead-letters")
    } else if !has_realtime_consumers {
        t!("dashboard-legacy-outbox-status-no-consumers")
    } else if pending > 0 {
        t!("dashboard-legacy-outbox-status-lagging")
    } else {
        t!("dashboard-legacy-outbox-status-ok")
    };
    let credentials = snapshot.credentials.data.as_ref();
    let delivery = snapshot.delivery.data.as_ref();
    let fulfillment = snapshot.fulfillment.data.as_ref();
    let recovery = snapshot.recovery.data.as_ref();
    let alerting = snapshot.alerting.data.as_ref();
    let worker_up = snapshot.worker.condition == "available";
    let audit_failures = snapshot.health.audit_failure_count;
    let audit_metrics_unavailable = snapshot
        .health
        .missing_services
        .iter()
        .any(|service| service == "public" || service == "worker");
    let audit_state = if audit_failures > 0 {
        "error"
    } else if audit_metrics_unavailable {
        "neutral"
    } else {
        "ok"
    };
    let audit_status = if audit_failures > 0 {
        t!("dashboard-legacy-audit-status-failures")
    } else if audit_metrics_unavailable {
        t!("dashboard-legacy-audit-status-unknown")
    } else {
        t!("dashboard-legacy-audit-status-ok")
    };
    let fulfillment_backlog = fulfillment.is_some_and(|data| {
        data.pending
            .saturating_add(data.blocked)
            .saturating_add(data.failed)
            .saturating_add(data.manual_reconcile)
            > 0
    });
    let fulfillment_state = if fulfillment.is_none() {
        "neutral"
    } else if fulfillment_backlog {
        "error"
    } else if snapshot.fulfillment.status == "ok" {
        "ok"
    } else {
        "neutral"
    };
    let recovery_actionable = recovery.is_some_and(|data| {
        data.payment_overdue_count
            .saturating_add(data.relist_pending_count)
            .saturating_add(data.failed_action_count)
            .saturating_add(data.manual_reconcile_count)
            > 0
    });
    let recovery_evidence_unavailable = recovery.is_some_and(|data| {
        data.worlds.iter().any(|world| {
            matches!(world.balance_status.as_str(), "unknown" | "unavailable")
                || matches!(world.evidence_status.as_str(), "unknown" | "unavailable")
        })
    });
    let recovery_evidence_warning = recovery.is_some_and(|data| {
        data.worlds.iter().any(|world| {
            matches!(world.balance_status.as_str(), "low" | "stale")
                || world.evidence_status == "stale"
        })
    });
    let recovery_state = if recovery.is_none() {
        "neutral"
    } else if recovery_actionable {
        "error"
    } else if recovery_evidence_unavailable {
        "neutral"
    } else if recovery_evidence_warning {
        "warn"
    } else if snapshot.recovery.status == "ok" {
        "ok"
    } else {
        "warn"
    };
    let recovery_status = if recovery.is_none() || recovery_evidence_unavailable {
        t!("dashboard-legacy-recovery-status-unavailable")
    } else if recovery_state == "ok" {
        t!("dashboard-legacy-recovery-status-ok")
    } else {
        t!("dashboard-legacy-recovery-status-degraded")
    };
    let mut sorted_availability =
        credentials.map(|data| data.availability.clone()).unwrap_or_default();
    sorted_availability.sort_by(|left, right| match (left.world.as_str(), right.world.as_str()) {
        ("main", "main") => std::cmp::Ordering::Equal,
        ("main", _) => std::cmp::Ordering::Less,
        (_, "main") => std::cmp::Ordering::Greater,
        _ => left.world.cmp(&right.world),
    });

    rsx! {
        Fragment {
            section {
                class: "admin-dashboard-shortcuts",
                aria_labelledby: "dashboard-shortcuts-title",
                h2 { id: "dashboard-shortcuts-title", {t!("dashboard-shortcuts-title")} }
                p { class: "admin-dashboard-status", {t!("dashboard-shortcuts-note")} }
                nav { class: "admin-quick-links", aria_label: t!("dashboard-shortcuts-title"),
                    Link { class: "admin-quick-link", to: Route::Auctions {},
                        span { class: "admin-quick-link__icon" }
                        span { class: "admin-quick-link__copy",
                            strong { {t!("dashboard-open-auctions")} }
                            small { {t!("dashboard-open-auctions-note")} }
                        }
                    }
                    Link { class: "admin-quick-link", to: Route::Mails {},
                        span { class: "admin-quick-link__icon admin-quick-link__icon--mail" }
                        span { class: "admin-quick-link__copy",
                            strong { {t!("dashboard-open-mail")} }
                            small { {t!("dashboard-open-mail-note")} }
                        }
                    }
                    Link { class: "admin-quick-link", to: Route::Deliveries {},
                        span { class: "admin-quick-link__icon admin-quick-link__icon--delivery" }
                        span { class: "admin-quick-link__copy",
                            strong { {t!("dashboard-open-deliveries")} }
                            small { {t!("dashboard-open-deliveries-note")} }
                        }
                    }
                    Link { class: "admin-quick-link", to: Route::Outbox {},
                        span { class: "admin-quick-link__icon admin-quick-link__icon--status" }
                        span { class: "admin-quick-link__copy",
                            strong { {t!("dashboard-open-status")} }
                            small { {t!("dashboard-open-status-note")} }
                        }
                    }
                }
            }

            div { class: "admin-dashboard-cards",
                {semantic_card(
                    "dashboard-worker-channel",
                    t!("dashboard-legacy-worker-channel"),
                    snapshot.worker.condition.clone(),
                    badge(
                        if worker_up { "ok" } else { "error" },
                        if worker_up {
                            t!("dashboard-legacy-channel-up")
                        } else {
                            t!("dashboard-legacy-channel-down")
                        },
                    ),
                )}
                {semantic_card(
                    "dashboard-worker-serving",
                    t!("dashboard-legacy-serving"),
                    if snapshot.worker.ready { "ready" } else { "not-ready" }.to_owned(),
                    text(if snapshot.worker.ready {
                        t!("dashboard-legacy-yes")
                    } else {
                        t!("dashboard-legacy-no")
                    }),
                )}
                {card(
                    t!("dashboard-legacy-poll-status"),
                    badge(
                        if poll_paused { "neutral" } else { "ok" },
                        if poll_paused {
                            t!("dashboard-legacy-poll-paused")
                        } else {
                            t!("dashboard-legacy-poll-running")
                        },
                    ),
                )}
            }

            h2 { {t!("dashboard-legacy-audit-health")} }
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
                    api: availability(&snapshot, "public"),
                    worker: availability(&snapshot, "worker")
                )}
            }
            p { class: "admin-dashboard-status", {t!("dashboard-legacy-audit-scope")} }
            if audit_failures == 0 && !audit_metrics_unavailable {
                p { class: "admin-dashboard-status", {t!("dashboard-legacy-audit-no-failures")} }
            }

            h2 { {t!("dashboard-legacy-tracing-health")} }
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

            h2 { {t!("dashboard-legacy-outbox-health")} }
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
                    t!("dashboard-legacy-outbox-retry"),
                    text(t!("dashboard-legacy-metric-not-collected")),
                )}
                {card(
                    t!("dashboard-legacy-outbox-dead-letter"),
                    text(if realtime.is_none() {
                        t!("dashboard-legacy-no-data")
                    } else {
                        failures.to_string()
                    }),
                )}
                {card(
                    t!("dashboard-legacy-outbox-claimed"),
                    text(t!("dashboard-legacy-metric-not-collected")),
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
            h3 { {t!("dashboard-legacy-outbox-attempts")} }
            p { class: "admin-dashboard-status", {t!("dashboard-legacy-metric-not-collected")} }

            h2 { {t!("dashboard-legacy-cookie-readiness-by-world")} }
            if credentials.is_none_or(|data| data.availability.is_empty()) {
                p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
            } else if let Some(credentials) = credentials {
                div { class: "admin-dashboard-cards",
                    for item in credentials.availability.iter() {
                        {
                            let (tone, status) = if item.available > 0 {
                                ("ok", t!("dashboard-legacy-cookie-ready"))
                            } else if item.invalid > 0 {
                                ("error", t!("dashboard-legacy-cookie-undecryptable"))
                            } else {
                                ("warn", t!("dashboard-legacy-cookie-no-active"))
                            };
                            let world = item.world.clone();
                            rsx! {
                                div { class: "admin-dashboard-card", key: "{world}",
                                    div { class: "admin-dashboard-value", {badge(tone, status)} }
                                    div { class: "admin-dashboard-label", "{world}" }
                                }
                            }
                        }
                    }
                }
            }

            h2 { {t!("dashboard-legacy-cookies-by-world")} }
            if sorted_availability.is_empty() {
                p { class: "admin-dashboard-status", {t!("dashboard-legacy-no-data")} }
            } else {
                div { class: "admin-dashboard-cards",
                    for item in sorted_availability.iter() {
                        {
                            let world = item.world.clone();
                            rsx! {
                                div { class: "admin-dashboard-card", key: "{world}",
                                    div { class: "admin-dashboard-value", "{item.available}" }
                                    div { class: "admin-dashboard-label", "{world}" }
                                }
                            }
                        }
                    }
                }
            }

            h2 { {t!("dashboard-legacy-deliveries-by-status")} }
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

            h2 { {t!("dashboard-legacy-fulfillment-health")} }
            if let Some(fulfillment) = fulfillment {
                div { class: "admin-dashboard-cards",
                    {card(
                        t!("dashboard-legacy-fulfillment-health"),
                        badge(fulfillment_state, fulfillment_status(fulfillment_state)),
                    )}
                    {card(
                        t!("dashboard-legacy-fulfillment-pending"),
                        text(fulfillment.pending.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-fulfillment-blocked"),
                        text(fulfillment.blocked.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-fulfillment-failed"),
                        text(fulfillment.failed.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-fulfillment-manual-reconcile"),
                        text(fulfillment.manual_reconcile.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-fulfillment-oldest-backlog"),
                        text(t!(
                            "dashboard-legacy-duration-seconds",
                            seconds: fulfillment.oldest_backlog_age_seconds
                        )),
                    )}
                }
            } else {
                p { class: "admin-dashboard-status",
                    {t!("dashboard-legacy-fulfillment-status-unavailable")}
                }
            }

            h2 { {t!("dashboard-legacy-recovery-health")} }
            if let Some(recovery) = recovery {
                div { class: "admin-dashboard-cards",
                    {card(
                        t!("dashboard-legacy-recovery-health"),
                        badge(recovery_state, recovery_status),
                    )}
                    {card(
                        t!("dashboard-legacy-recovery-payment-overdue"),
                        text(recovery.payment_overdue_count.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-recovery-relist-pending"),
                        text(recovery.relist_pending_count.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-recovery-failed-actions"),
                        text(recovery.failed_action_count.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-recovery-manual-reconcile"),
                        text(recovery.manual_reconcile_count.to_string()),
                    )}
                    {card(
                        t!("dashboard-legacy-recovery-oldest-overdue"),
                        text(t!(
                            "dashboard-legacy-recovery-age",
                            seconds: recovery.oldest_overdue_age_seconds
                        )),
                    )}
                }
                h3 { {t!("dashboard-legacy-recovery-worlds")} }
                div { class: "admin-dashboard-cards",
                    for world in recovery.worlds.iter() {
                        {
                            let balance_known = world.balance_observed_at.is_some()
                                && !matches!(world.balance_status.as_str(), "unknown" | "unavailable");
                            let balance_tone = match world.balance_status.as_str() {
                                "ok" => "ok",
                                "low" | "stale" => "warn",
                                _ => "neutral",
                            };
                            let evidence_tone = match world.evidence_status.as_str() {
                                "fresh" => "ok",
                                "stale" => "warn",
                                _ => "neutral",
                            };
                            let world_name = world.world.clone();
                            rsx! {
                                div { class: "admin-dashboard-card", key: "{world_name}",
                                    div { class: "admin-dashboard-value",
                                        span { class: "admin-dashboard-recovery-world",
                                            span {
                                                if balance_known {
                                                    {t!(
                                                        "dashboard-legacy-recovery-balance-credits",
                                                        credits: world.balance_credits
                                                    )}
                                                } else {
                                                    {t!("dashboard-legacy-no-data")}
                                                }
                                            }
                                            {badge(
                                                balance_tone,
                                                recovery_balance_status(&world.balance_status),
                                            )}
                                            span { class: "admin-dashboard-label",
                                                {t!("dashboard-legacy-recovery-evidence-label")}
                                                " "
                                                {badge(
                                                    evidence_tone,
                                                    recovery_evidence_status(&world.evidence_status),
                                                )}
                                            }
                                        }
                                    }
                                    div { class: "admin-dashboard-label", "{world_name}" }
                                }
                            }
                        }
                    }
                }
            } else {
                p { class: "admin-dashboard-status",
                    {t!("dashboard-legacy-recovery-status-unavailable")}
                }
            }

            h2 { {t!("dashboard-legacy-alerting-health")} }
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
            } else {
                p { class: "admin-dashboard-status",
                    {t!("dashboard-legacy-alerting-status-unavailable")}
                }
            }
        }
    }
}

fn card(label: String, value: Element) -> Element {
    rsx! {
        div { class: "admin-dashboard-card",
            div { class: "admin-dashboard-value", {value} }
            div { class: "admin-dashboard-label", "{label}" }
        }
    }
}

fn semantic_card(id: &'static str, label: String, state: String, value: Element) -> Element {
    rsx! {
        div { class: "admin-dashboard-card", id, "data-state": state,
            div { class: "admin-dashboard-value", {value} }
            div { class: "admin-dashboard-label", "{label}" }
        }
    }
}

fn badge(state: &'static str, value: String) -> Element {
    rsx! {
        span { class: "admin-dashboard-badge admin-dashboard-badge--{state}", "{value}" }
    }
}

fn text(value: String) -> Element {
    rsx! { "{value}" }
}

fn availability(snapshot: &AdminDashboardSnapshot, service: &str) -> String {
    if snapshot.health.missing_services.iter().any(|missing| missing == service) {
        t!("dashboard-legacy-audit-metrics-unavailable")
    } else {
        t!("dashboard-legacy-audit-metrics-available")
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

fn fulfillment_status(state: &str) -> String {
    match state {
        "ok" => t!("dashboard-legacy-fulfillment-status-ok"),
        "error" => t!("dashboard-legacy-fulfillment-status-degraded"),
        _ => t!("dashboard-legacy-fulfillment-status-unavailable"),
    }
}

fn recovery_balance_status(status: &str) -> String {
    match status {
        "ok" => t!("dashboard-legacy-recovery-balance-ok"),
        "low" => t!("dashboard-legacy-recovery-balance-low"),
        "stale" => t!("dashboard-legacy-recovery-balance-stale"),
        "unavailable" => t!("dashboard-legacy-recovery-balance-unavailable"),
        _ => t!("dashboard-legacy-recovery-balance-unknown"),
    }
}

fn recovery_evidence_status(status: &str) -> String {
    match status {
        "fresh" => t!("dashboard-legacy-recovery-evidence-fresh"),
        "stale" => t!("dashboard-legacy-recovery-evidence-stale"),
        "unavailable" => t!("dashboard-legacy-recovery-evidence-unavailable"),
        _ => t!("dashboard-legacy-recovery-evidence-unknown"),
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
