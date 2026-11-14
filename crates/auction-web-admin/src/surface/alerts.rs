//! Admin 告警台页面。
//! 活动/历史列表、确认与带到期的静默、处置手册锚点。静默只抑制重复通知，不修改阈值或
//! signal 状态；服务端仍是唯一判定者，页面只展示安全投影。

use auction_web_shared::{AsyncState, AsyncStateKind, Time, t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{
    AdminAlert, AdminAlertPage, acknowledge_admin_alert, fetch_admin_alerts, fetch_admin_webhooks,
    silence_admin_alert,
};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

const PAGE_SIZE: i64 = 25;
/// 公开仓库中的处置手册；Admin 不复制手册正文，只链接到固定锚点。
const RUNBOOK_BASE_URL: &str = "https://github.com/ngnlAYY/Auction/blob/main/ops/README.md";

pub(super) fn page() -> Element {
    let status = use_signal(|| "active".to_owned());
    let current_page = use_signal(|| 1_i64);
    let silence_hours = use_signal(|| 24_i64);
    let reason = use_signal(String::new);
    let mut refresh = use_signal(|| 0_u64);
    let busy = use_signal(|| None::<String>);
    let notice = use_signal(|| None::<AdminActionFeedback>);

    let requested_status = status();
    let requested_page = current_page();
    let alerts = use_resource(move || {
        let _ = refresh();
        let requested = (requested_status.clone(), requested_page);
        async move {
            let result = fetch_admin_alerts(&requested.0, "", requested.1, PAGE_SIZE).await;
            (requested, result)
        }
    });
    let state = alerts.read_unchecked();
    let content = match &*state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("alert-console-loading"),
                detail: "",
            }
        },
        Some((_, Err(error))) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(((loaded_status, loaded_page), Ok(data)))
            if loaded_status == &status() && *loaded_page == current_page() =>
        {
            let mut controller = status;
            let mut pager = current_page;
            let alert_page: AdminAlertPage = data.clone();
            rsx! {
                AlertTable {
                    page: alert_page,
                    busy: busy,
                    reason: reason,
                    silence_hours: silence_hours,
                    refresh,
                    notice,
                }
                WebhookTable { refresh }
                div { class: "admin-pagination",
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: current_page() <= 1,
                        onclick: move |_| pager.set((pager() - 1).max(1)),
                        {t!("admin-pagination-previous")}
                    }
                    span { class: "admin-pagination__summary",
                        {t!("alert-console-page", page: current_page())}
                    }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: current_page().saturating_mul(PAGE_SIZE) >= data_total(data),
                        onclick: move |_| pager.set(pager() + 1),
                        {t!("admin-pagination-next")}
                    }
                }
                div { class: "admin-alert-console__filters",
                    label {
                        {t!("alert-console-status")}
                        select {
                            value: status(),
                            onchange: move |event| {
                                let value = event.value();
                                controller.set(value.clone());
                                if !matches!(value.as_str(), "active" | "resolved" | "all") {
                                    controller.set("active".to_owned());
                                }
                                // 改变筛选回到第一页，避免落在不存在的深页。
                                pager.set(1);
                            },
                            option { value: "active", {t!("alert-console-status-active")} }
                            option { value: "resolved", {t!("alert-console-status-resolved")} }
                            option { value: "all", {t!("alert-console-status-all")} }
                        }
                    }
                }
            }
        }
        Some(_) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("alert-console-loading"),
                detail: "",
            }
        },
    };

    rsx! {
        section { class: "admin-alert-console-page", aria_labelledby: "alert-console-title",
            h1 { id: "alert-console-title", {t!("page-alerts-title")} }
            p { class: "admin-alert-console__hint", {t!("alert-console-hint")} }
            if let Some(feedback) = notice() {
                p {
                    class: "admin-shell__banner",
                    role: "status",
                    if feedback.is_success() {
                        {t!("alert-console-action-ok")}
                    } else {
                        {t!("alert-console-action-failed")}
                    }
                }
            }
            {content}
        }
    }
}

fn data_total(page: &AdminAlertPage) -> i64 {
    page.total
}

#[component]
fn AlertTable(
    page: AdminAlertPage,
    busy: Signal<Option<String>>,
    reason: Signal<String>,
    silence_hours: Signal<i64>,
    refresh: Signal<u64>,
    notice: Signal<Option<AdminActionFeedback>>,
) -> Element {
    if page.alerts.is_empty() {
        return rsx! {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("alert-console-empty"),
                detail: "",
            }
        };
    }
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-alert-console__table",
                aria_label: t!("page-alerts-title"),
                thead {
                    tr {
                        th { scope: "col", {t!("alert-console-kind")} }
                        th { scope: "col", {t!("alert-console-scope")} }
                        th { scope: "col", {t!("alert-console-count")} }
                        th { scope: "col", {t!("alert-console-first-seen")} }
                        th { scope: "col", {t!("alert-console-last-seen")} }
                        th { scope: "col", {t!("alert-console-state")} }
                        th { scope: "col", {t!("alert-console-runbook")} }
                        th { scope: "col", {t!("alert-console-actions")} }
                    }
                }
                tbody {
                    for alert in page.alerts.iter().cloned() {
                        tr { key: "{alert.id}", class: "admin-alert-console__row",
                            td {
                                div { class: "admin-alert-console__kind", "{alert.kind}" }
                                span {
                                    class: if alert.severity == "urgent" {
                                        "admin-dashboard-badge admin-dashboard-badge--error"
                                    } else {
                                        "admin-dashboard-badge admin-dashboard-badge--neutral"
                                    },
                                    {severity_label(&alert)}
                                }
                            }
                            td { class: "admin-alert-console__scope", "{alert.source} / {alert.scope}" }
                            td { class: "admin-alert-console__count", "{alert.count}" }
                            td { Time { value: alert.first_seen_at.clone() } }
                            td { Time { value: alert.last_seen_at.clone() } }
                            td {
                                div {
                                    class: if alert.active {
                                        "admin-dashboard-badge admin-dashboard-badge--warn"
                                    } else {
                                        "admin-dashboard-badge admin-dashboard-badge--ok"
                                    },
                                    if alert.active {
                                        {t!("alert-console-active")}
                                    } else {
                                        {t!("alert-console-resolved")}
                                    }
                                }
                                if let Some(resolved_at) = alert.resolved_at.clone() {
                                    div { Time { value: resolved_at } }
                                }
                                if let Some(acknowledged_at) = alert.acknowledged_at.clone() {
                                    div { class: "admin-alert-console__ack",
                                        {t!("alert-console-acknowledged")}
                                        " "
                                        Time { value: acknowledged_at }
                                        if let Some(actor) = alert.acknowledged_by.clone() {
                                            " "
                                            span { class: "admin-alert-console__actor", "{actor}" }
                                        }
                                    }
                                }
                                if let Some(silenced_until) = alert.silenced_until.clone() {
                                    div { class: "admin-alert-console__silence",
                                        {t!("alert-console-silenced-until")}
                                        " "
                                        Time { value: silenced_until }
                                    }
                                }
                            }
                            td {
                                a {
                                    class: "admin-alert-console__anchor",
                                    href: format!(
                                        "{RUNBOOK_BASE_URL}#{}",
                                        alert.runbook_anchor
                                    ),
                                    target: "_blank",
                                    rel: "noopener noreferrer",
                                    "{alert.runbook_anchor}"
                                }
                            }
                            td { class: "admin-alert-console__actions",
                                if alert.active {
                                    div { class: "admin-alert-console__action-group",
                                        button {
                                            class: "admin-btn-secondary",
                                            r#type: "button",
                                            disabled: busy().is_some(),
                                            onclick: {
                                                let alert = alert.clone();
                                                move |_| {
                                                    start_acknowledge(alert.clone(), busy, reason, refresh, notice);
                                                }
                                            },
                                            {t!("alert-console-acknowledge")}
                                        }
                                        select {
                                            value: silence_hours().to_string(),
                                            onchange: move |event| {
                                                if let Ok(hours) = event.value().parse::<i64>()
                                                    && (1..=72).contains(&hours)
                                                {
                                                    let mut signal = silence_hours;
                                                    signal.set(hours);
                                                }
                                            },
                                            option { value: "1", {t!("alert-console-1h")} }
                                            option { value: "8", {t!("alert-console-8h")} }
                                            option { value: "24", {t!("alert-console-24h")} }
                                            option { value: "72", {t!("alert-console-72h")} }
                                        }
                                        button {
                                            class: "admin-btn-secondary",
                                            r#type: "button",
                                            disabled: busy().is_some(),
                                            onclick: {
                                                let alert = alert.clone();
                                                move |_| {
                                                    start_silence(alert.clone(), silence_hours(), busy, reason, refresh, notice);
                                                }
                                            },
                                            {t!("alert-console-silence")}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "admin-alert-console__reason",
                label {
                    {t!("alert-console-reason")}
                    input {
                        r#type: "text",
                        maxlength: "2048",
                        value: reason(),
                        oninput: move |event| {
                            let value = event.value();
                            if value.chars().count() <= 2048 {
                                let mut signal = reason;
                                signal.set(value);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn WebhookTable(refresh: Signal<u64>) -> Element {
    let webhooks = use_resource(move || {
        let _ = refresh();
        async move { fetch_admin_webhooks(50).await }
    });
    let state = webhooks.read_unchecked();
    match &*state {
        None => rsx! {
            p { class: "admin-shell__muted", {t!("alert-webhooks-loading")} }
        },
        Some(Err(_)) => rsx! {
            p { class: "admin-shell__muted", {t!("alert-webhooks-unavailable")} }
        },
        Some(Ok(entries)) if entries.is_empty() => rsx! {
            section { class: "admin-alert-console__webhooks",
                h3 { {t!("alert-webhooks-title")} }
                p { class: "admin-shell__muted", {t!("alert-webhooks-empty")} }
            }
        },
        Some(Ok(entries)) => rsx! {
            section { class: "admin-alert-console__webhooks",
                h3 { {t!("alert-webhooks-title")} }
                div { class: "admin-shell__table-wrap",
                    table { class: "admin-shell__table",
                        aria_label: t!("alert-webhooks-title"),
                        thead {
                            tr {
                                th { scope: "col", {t!("alert-webhooks-status")} }
                                th { scope: "col", {t!("alert-webhooks-kind")} }
                                th { scope: "col", {t!("alert-webhooks-attempts")} }
                                th { scope: "col", {t!("alert-webhooks-next")} }
                                th { scope: "col", {t!("alert-webhooks-failure")} }
                                th { scope: "col", {t!("alert-webhooks-created")} }
                            }
                        }
                        tbody {
                            for entry in entries.iter().cloned() {
                                tr { key: "{entry.id}",
                                    td { {webhook_status_label(&entry.status)} }
                                    td { "{entry.kind} / {entry.source} / {entry.scope}" }
                                    td { "{entry.attempt_count}" }
                                    td {
                                        if let Some(next) = entry.next_attempt_at {
                                            Time { value: next }
                                        }
                                    }
                                    td { {entry.failure_code.unwrap_or_default()} }
                                    td { Time { value: entry.created_at } }
                                }
                            }
                        }
                    }
                }
            }
        },
    }
}

/// Webhook 状态是数据库闭集；界面复用投递状态译文，未知值保留原码供诊断。
fn webhook_status_label(status: &str) -> String {
    match status {
        "pending" => t!("delivery-status-pending"),
        "sending" => t!("delivery-status-sending"),
        "succeeded" => t!("delivery-status-succeeded"),
        "failed" => t!("delivery-status-failed"),
        "manual_reconcile" => t!("delivery-status-manual"),
        other => other.to_owned(),
    }
}

fn severity_label(alert: &AdminAlert) -> String {
    if alert.severity == "digest" {
        t!("alert-console-severity-digest")
    } else {
        t!("alert-console-severity-urgent")
    }
}

fn start_acknowledge(
    alert: AdminAlert,
    mut busy: Signal<Option<String>>,
    reason: Signal<String>,
    mut refresh: Signal<u64>,
    mut notice: Signal<Option<AdminActionFeedback>>,
) {
    if busy().is_some() {
        return;
    }
    busy.set(Some(format!("acknowledge:{}", alert.id)));
    notice.set(None);
    spawn(async move {
        let result = acknowledge_admin_alert(Uuid::new_v4(), &alert.id, reason().trim()).await;
        busy.set(None);
        match result {
            Ok(_) => {
                notice.set(Some(AdminActionFeedback::Success));
                refresh.set(refresh().saturating_add(1));
            }
            Err(failure) => notice.set(Some(AdminActionFeedback::Error(failure.code))),
        }
    });
}

fn start_silence(
    alert: AdminAlert,
    hours: i64,
    mut busy: Signal<Option<String>>,
    reason: Signal<String>,
    mut refresh: Signal<u64>,
    mut notice: Signal<Option<AdminActionFeedback>>,
) {
    if busy().is_some() {
        return;
    }
    let seconds = hours.saturating_mul(3_600);
    busy.set(Some(format!("silence:{}", alert.id)));
    notice.set(None);
    spawn(async move {
        let result = silence_admin_alert(Uuid::new_v4(), &alert.id, seconds, reason().trim()).await;
        busy.set(None);
        match result {
            Ok(_) => {
                notice.set(Some(AdminActionFeedback::Success));
                refresh.set(refresh().saturating_add(1));
            }
            Err(failure) => notice.set(Some(AdminActionFeedback::Error(failure.code))),
        }
    });
}
