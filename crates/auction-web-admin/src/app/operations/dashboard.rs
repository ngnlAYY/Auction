//! Admin 管理总览页面。
//!
//! 总览只聚合各个已冻结 HTTP 投影的安全摘要，不创建新的后端状态，也不把失败的请求伪装
//! 成零值。页面因此可以在某个能力尚未部署或暂时不可用时，仍然明确呈现该能力的状态。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    AdminAlertingSection, AdminAuctionPage, AdminCredentialData, AdminCredentialScope,
    AdminCredentialSection, AdminDashboardSnapshot, AdminMailData, AdminMailIntakePage,
    AdminMailIntakeQuery, AdminMailPolling, AdminMailSection, AdminRealtimeData, DeliveryPage,
    fetch_admin_auctions, fetch_dashboard, fetch_deliveries, fetch_mail_intakes, set_mail_paused,
    trigger_mail_poll,
};

use super::{Route, format_admin_time};

#[derive(Clone, PartialEq)]
struct DashboardMetric {
    value: String,
    state: String,
    detail: String,
    tone: &'static str,
}

/// 加载管理总览所需的各类安全摘要。
pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let dashboard = use_resource(move || {
        let _ = refresh();
        fetch_dashboard()
    });
    let auctions = use_resource(move || {
        let _ = refresh();
        fetch_admin_auctions("all".to_owned(), "all".to_owned(), 1)
    });
    let deliveries = use_resource(move || {
        let _ = refresh();
        fetch_deliveries(String::new(), 1)
    });
    let mail_intakes = use_resource(move || {
        let _ = refresh();
        // dashboard 统计所有已配置邮箱，不能固定读取 main，否则 isekai 等邮箱的入站事实会被漏算。
        fetch_mail_intakes(AdminMailIntakeQuery::default(), 1)
    });

    let dashboard_state = dashboard.read_unchecked();
    let auction_state = auctions.read_unchecked();
    let delivery_state = deliveries.read_unchecked();
    let mail_intake_state = mail_intakes.read_unchecked();
    let mail_dashboard_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.mail.clone()),
        Err(error) => Err(error.clone()),
    });
    let realtime_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => snapshot
            .realtime
            .data
            .clone()
            .ok_or_else(|| section_unavailable(snapshot.realtime.error_code.as_deref())),
        Err(error) => Err(error.clone()),
    });
    let alerting_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.alerting.clone()),
        Err(error) => Err(error.clone()),
    });
    let credentials_state = dashboard_state.as_ref().map(|result| match result {
        Ok(snapshot) => Ok(snapshot.credentials.clone()),
        Err(error) => Err(error.clone()),
    });

    let auction_metric = auction_metric(&auction_state);
    let delivery_metric = delivery_metric(&delivery_state);
    let realtime_metric = realtime_metric(&realtime_state);
    let mail_metric = mail_metric(&mail_dashboard_state, &mail_intake_state);
    let alerting_metric = alerting_metric(&alerting_state);
    let credentials_metric = credential_metric(&credentials_state);
    let mail_pollers = mail_dashboard_state
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .and_then(|section| section.data.as_ref())
        .map(|data| data.pollers.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "admin-dashboard",
            header { class: "admin-dashboard__hero",
                div { class: "admin-dashboard__hero-copy",
                    h1 { {t!("page-dashboard-title")} }
                    p { class: "admin-dashboard__subtitle", {t!("dashboard-subtitle")} }
                }
                div { class: "admin-dashboard__hero-actions",
                    span { class: "admin-dashboard__live-state",
                        span { class: "admin-dashboard__live-dot", aria_hidden: "true" }
                        {t!("dashboard-connection")}
                    }
                    button {
                        class: "admin-dashboard__refresh",
                        r#type: "button",
                        onclick: move |_| refresh.set(refresh().saturating_add(1)),
                        span { class: "admin-dashboard__refresh-icon", aria_hidden: "true" }
                        {t!("dashboard-refresh")}
                    }
                }
            }

            section { class: "admin-metric-grid", aria_labelledby: "dashboard-metrics-title",
                h2 { id: "dashboard-metrics-title", class: "admin-visually-hidden", {t!("dashboard-metrics-title")} }
                MetricCard { label: t!("dashboard-auctions-label"), metric: auction_metric.clone() }
                MetricCard { label: t!("dashboard-deliveries-label"), metric: delivery_metric.clone() }
                MetricCard { label: t!("dashboard-realtime-label"), metric: realtime_metric.clone() }
                MetricCard { label: t!("dashboard-mail-label"), metric: mail_metric.clone() }
                MetricCard { label: t!("dashboard-credentials-label"), metric: credentials_metric.clone() }
                MetricCard { label: t!("dashboard-alerting-label"), metric: alerting_metric.clone() }
            }

            div { class: "admin-dashboard__layout",
                section { class: "admin-dashboard__card", aria_labelledby: "dashboard-signals-title",
                    header { class: "admin-dashboard__card-header",
                        div {
                            p { class: "admin-dashboard__card-kicker", {t!("dashboard-signal-kicker")} }
                            h2 { id: "dashboard-signals-title", {t!("dashboard-signals-title")} }
                        }
                        span { class: "admin-dashboard__readonly", {t!("dashboard-readonly")} }
                    }
                    p { class: "admin-dashboard__card-note", {t!("dashboard-signals-note")} }
                    div { class: "admin-dashboard__status-list",
                        DashboardStatusRow { label: t!("dashboard-auctions-label"), metric: auction_metric }
                        DashboardStatusRow { label: t!("dashboard-deliveries-label"), metric: delivery_metric }
                        DashboardStatusRow { label: t!("dashboard-realtime-label"), metric: realtime_metric }
                        DashboardStatusRow { label: t!("dashboard-mail-label"), metric: mail_metric }
                        DashboardStatusRow { label: t!("dashboard-credentials-label"), metric: credentials_metric }
                        DashboardStatusRow { label: t!("dashboard-alerting-label"), metric: alerting_metric }
                    }
                }

                section { class: "admin-dashboard__card", aria_labelledby: "dashboard-shortcuts-title",
                    header { class: "admin-dashboard__card-header",
                        div {
                            p { class: "admin-dashboard__card-kicker", {t!("dashboard-shortcuts-kicker")} }
                            h2 { id: "dashboard-shortcuts-title", {t!("dashboard-shortcuts-title")} }
                        }
                    }
                    p { class: "admin-dashboard__card-note", {t!("dashboard-shortcuts-note")} }
                    nav { class: "admin-quick-links", aria_label: t!("dashboard-shortcuts-title"),
                        Link { class: "admin-quick-link", to: Route::Auctions {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--auction", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-auctions")} }
                                small { {t!("dashboard-open-auctions-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Mails {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--mail", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-mail")} }
                                small { {t!("dashboard-open-mail-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Deliveries {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--delivery", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-deliveries")} }
                                small { {t!("dashboard-open-deliveries-note")} }
                            }
                        }
                        Link { class: "admin-quick-link", to: Route::Status {},
                            span { class: "admin-quick-link__icon admin-quick-link__icon--status", aria_hidden: "true" }
                            span { class: "admin-quick-link__copy",
                                strong { {t!("dashboard-open-status")} }
                                small { {t!("dashboard-open-status-note")} }
                            }
                        }
                    }
                }
            }

            if !mail_pollers.is_empty() {
                MailPollerControls { pollers: mail_pollers, refresh }
            }

            if let Some(Ok(snapshot)) = dashboard_state.as_ref() {
                DashboardRuntimeDetails { snapshot: snapshot.clone() }
            }

            p { class: "admin-dashboard__source-note", {t!("dashboard-source-note")} }
        }
    }
}

#[component]
fn MailPollerControls(pollers: Vec<AdminMailPolling>, mut refresh: Signal<u64>) -> Element {
    let mut busy = use_signal(|| false);
    let notice = use_signal(|| None::<bool>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    rsx! {
        section {
            class: "admin-panel admin-dashboard__mail-controls",
            aria_labelledby: "dashboard-mail-pollers-title",
            h2 { id: "dashboard-mail-pollers-title", {t!("admin-mail-polling-title")} }
            div { class: "admin-panel__actions",
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        if busy() {
                            return;
                        }
                        busy.set(true);
                        let command_id = stable_command_id(command_state, "trigger".to_owned());
                        let mut notice = notice;
                        let mut refresh = refresh;
                        let mut busy = busy;
                        spawn(async move {
                            if trigger_mail_poll(command_id).await.is_ok() {
                                notice.set(Some(true));
                                command_state.set(None);
                                refresh.set(refresh().saturating_add(1));
                            } else {
                                notice.set(Some(false));
                            }
                            busy.set(false);
                        });
                    },
                    {t!("admin-mail-trigger")}
                }
                for polling in pollers.iter().cloned() {
                    {
                        let desired_paused = !polling.paused;
                        let label = if polling.paused {
                            t!("admin-mail-resume")
                        } else {
                            t!("admin-mail-pause")
                        };
                        let mailbox_key = polling.mailbox_key.clone();
                        rsx! {
                            div { class: "admin-dashboard__mail-poller", key: "{mailbox_key}",
                                p { class: "admin-panel__note",
                                    {t!("admin-mail-polling-summary",
                                        mailbox: mailbox_key.clone(),
                                        status: if polling.paused {
                                            t!("admin-mail-paused")
                                        } else {
                                            t!("admin-mail-running")
                                        },
                                        version: polling.state_version,
                                        failures: polling.consecutive_failures
                                    )}
                                }
                                button {
                                    class: "admin-btn-secondary",
                                    r#type: "button",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        if busy() {
                                            return;
                                        }
                                        busy.set(true);
                                        let polling = polling.clone();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("pause:{}:{}:{}", polling.mailbox_key, polling.state_version, desired_paused),
                                        );
                                        let mut notice = notice;
                                        let mut refresh = refresh;
                                        let mut busy = busy;
                                        spawn(async move {
                                            match set_mail_paused(command_id, polling, desired_paused).await {
                                                Ok(_) => {
                                                    notice.set(Some(true));
                                                    command_state.set(None);
                                                    refresh.set(refresh().saturating_add(1));
                                                }
                                                Err(_) => notice.set(Some(false)),
                                            }
                                            busy.set(false);
                                        });
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(success) = notice() {
                p {
                    class: if success { "admin-panel__note" } else { "admin-panel__note admin-panel__note--error" },
                    role: "status",
                    {t!(if success { "admin-mail-action-success" } else { "admin-mail-action-error" })}
                }
            }
        }
    }
}

fn stable_command_id(mut command_state: Signal<Option<(String, Uuid)>>, signature: String) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

/// 展示后端已经提供的完整运维安全投影，避免 Dashboard 只显示摘要卡而隐藏旧版可见的健康
/// 证据。这里不执行额外命令，也不把外部凭据或数据库明细带入浏览器。
#[component]
fn DashboardRuntimeDetails(snapshot: AdminDashboardSnapshot) -> Element {
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

fn section_unavailable(error_code: Option<&str>) -> String {
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

#[component]
fn MetricCard(label: String, metric: DashboardMetric) -> Element {
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
fn DashboardStatusRow(label: String, metric: DashboardMetric) -> Element {
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

fn auction_metric(state: &Option<Result<AdminAuctionPage, String>>) -> DashboardMetric {
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

fn delivery_metric(state: &Option<Result<DeliveryPage, String>>) -> DashboardMetric {
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

fn realtime_metric(state: &Option<Result<AdminRealtimeData, String>>) -> DashboardMetric {
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

fn mail_metric(
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

fn alerting_metric(state: &Option<Result<AdminAlertingSection, String>>) -> DashboardMetric {
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

fn credential_metric(state: &Option<Result<AdminCredentialSection, String>>) -> DashboardMetric {
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
