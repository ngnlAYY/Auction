//! Admin 告警台页面。
//! 活动/历史列表、确认与带到期的静默、处置手册锚点。静默只抑制重复通知，不修改阈值或
//! signal 状态；服务端仍是唯一判定者，页面只展示安全投影。

use auction_web_shared::{AsyncState, AsyncStateKind, Time, t};
use dioxus::prelude::*;

use self::reason::{
    AlertReasonDraft, AlertReasonInput, use_alert_reason_draft, valid_alert_reason_length,
};
use super::pagination::{AdminPager, use_admin_page, use_admin_pagination};
use super::pending::{PendingCommandPanel, PendingCommands, set_if_alive, use_pending_commands};
use super::query::{AlertsQuery, alert_status_choice, use_alerts_query_state};
use super::{AdminActionFeedback, admin_api_error_message};
use crate::api::{
    AdminAlert, AdminAlertPage, acknowledge_admin_alert, fetch_admin_alerts, fetch_admin_webhooks,
    silence_admin_alert,
};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod page_tests;
mod reason;

/// 公开仓库中的处置手册；Admin 不复制手册正文，只链接到固定锚点。
const RUNBOOK_BASE_URL: &str = "https://github.com/ngnlAYY/Auction/blob/main/ops/README.md";
/// 行内静默时长选项（小时），均在服务端 60 秒至 7 天的边界内。
const SILENCE_HOUR_OPTIONS: [i64; 4] = [1, 8, 24, 72];
const DEFAULT_SILENCE_HOURS: i64 = 24;

pub(super) fn page() -> Element {
    // 状态筛选与分页跟地址栏双向同步；备注、静默时长与命令反馈是瞬态，不入 URL。
    let query = use_alerts_query_state();
    let query_state = query.state;
    let mut refresh = use_signal(|| 0_u64);
    let notice = use_signal(|| None::<AdminActionFeedback>);
    let reason = use_alert_reason_draft();
    // 命令成功后宿主递增 refresh，列表与 webhook 视图随之重读。
    let pending = use_pending_commands(refresh, |entity| entity.starts_with("alert:"));
    let pagination = use_admin_pagination("alerts", &["page_size"], Some(100), 100_000);
    let alerts = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            query_state()
        },
        |query: AlertsQuery, size| fetch_admin_alerts(query.status, query.page, size),
    );
    let content = match &*alerts.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("alert-console-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            rsx! {
                AlertTable { page: page.clone(), reason, notice, refresh }
                AdminPager {
                    pagination,
                    page: page.page,
                    page_size: page.page_size,
                    total: Some(page.total),
                    label: t!("page-alerts-title"),
                    on_page_change: move |next_page| {
                        on_page_change.push(AlertsQuery { page: next_page, ..query_state() });
                    },
                }
            }
        }
    };
    let mut status_query = query;

    rsx! {
        section { class: "admin-alert-console-page", aria_labelledby: "alert-console-title",
            h1 { id: "alert-console-title", {t!("page-alerts-title")} }
            p { class: "admin-alert-console__hint", {t!("alert-console-hint")} }
            PendingCommandPanel { owner: pending }
            div { class: "admin-alert-console__filters",
                label {
                    {t!("alert-console-status")}
                    select {
                        value: query_state().status,
                        onchange: move |event| {
                            // 未知值不改变筛选；切换筛选回到第一页，避免落在不存在的深页。
                            if let Some(status) = alert_status_choice(&event.value()) {
                                status_query.push(AlertsQuery { status, page: 1 });
                            }
                        },
                        option { value: "active", {t!("alert-console-status-active")} }
                        option { value: "resolved", {t!("alert-console-status-resolved")} }
                        option { value: "all", {t!("alert-console-status-all")} }
                    }
                }
            }
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "action-feedback action-feedback--success"
                    } else {
                        "action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("alert-console-action-ok"))}
                }
            }
            {content}
            WebhookTable { refresh }
        }
    }
}

#[component]
fn AlertTable(
    page: AdminAlertPage,
    reason: AlertReasonDraft,
    notice: Signal<Option<AdminActionFeedback>>,
    refresh: Signal<u64>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let mut focus_reason = use_signal(|| false);
    let on_action = use_callback(move |command: AlertCommand| {
        if !valid_alert_reason_length(command.reason()) {
            reason.mark_invalid();
            focus_reason.set(true);
            return;
        }
        submit_alert_command(pending, notice, refresh, command);
    });
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
                    for alert in page.alerts {
                        AlertRow { key: "{alert.id}", alert, reason: reason.value, on_action }
                    }
                }
            }
            AlertReasonInput { draft: reason, focus_requested: focus_reason }
        }
    }
}

/// 单条告警；静默时长是本行草稿，点击时与共享备注一起冻结进命令负载。
#[component]
fn AlertRow(
    alert: AdminAlert,
    reason: Signal<String>,
    on_action: EventHandler<AlertCommand>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let mut silence_hours = use_signal(|| DEFAULT_SILENCE_HOURS);
    // 同一告警最多保留一个已提交意图；结果未知时只能在未决区按原 command_id 重试。
    let blocked = pending.contains(&alert_command_entity(&alert.id));
    let acknowledge_id = alert.id.clone();
    let silence_id = alert.id.clone();
    rsx! {
        tr {
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
                            span { "{actor}" }
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
                    href: format!("{RUNBOOK_BASE_URL}#{}", alert.runbook_anchor),
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "{alert.runbook_anchor}"
                }
            }
            td {
                if let Some(incident_generation) = alert.incident_generation.filter(|_| alert.active) {
                    div { class: "admin-alert-console__action-group",
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: blocked,
                            onclick: move |_| {
                                on_action.call(AlertCommand::acknowledge(&acknowledge_id, incident_generation, &reason()));
                            },
                            {t!("alert-console-acknowledge")}
                        }
                        // 行内没有可见标签：可访问名称说明它选的是静默时长，旁边的按钮不能代为命名。
                        select {
                            aria_label: t!("alert-console-silence-duration"),
                            value: silence_hours().to_string(),
                            onchange: move |event| {
                                if let Ok(hours) = event.value().parse::<i64>()
                                    && SILENCE_HOUR_OPTIONS.contains(&hours)
                                {
                                    silence_hours.set(hours);
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
                            disabled: blocked,
                            onclick: move |_| {
                                on_action.call(AlertCommand::silence(&silence_id, incident_generation, silence_hours(), &reason()));
                            },
                            {t!("alert-console-silence")}
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
            p { class: "admin-panel__note", {t!("alert-webhooks-loading")} }
        },
        Some(Err(_)) => rsx! {
            p { class: "admin-panel__note", {t!("alert-webhooks-unavailable")} }
        },
        Some(Ok(entries)) if entries.is_empty() => rsx! {
            section {
                h3 { {t!("alert-webhooks-title")} }
                p { class: "admin-panel__note", {t!("alert-webhooks-empty")} }
            }
        },
        Some(Ok(entries)) => rsx! {
            section {
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

/// 点击时冻结本行的事件代数和表单参数。首次提交与原样重试共用这份负载，
/// 列表重读不能替换未决命令的事件代数，避免将旧意图落到复发后的告警。
#[derive(Clone, Debug, PartialEq, Eq)]
enum AlertCommand {
    Acknowledge { alert_id: String, incident_generation: i64, reason: String },
    Silence { alert_id: String, incident_generation: i64, reason: String, silence_seconds: i64 },
}

impl AlertCommand {
    fn acknowledge(alert_id: &str, incident_generation: i64, reason: &str) -> Self {
        Self::Acknowledge {
            alert_id: alert_id.to_owned(),
            incident_generation,
            reason: reason.trim().to_owned(),
        }
    }

    fn silence(alert_id: &str, incident_generation: i64, hours: i64, reason: &str) -> Self {
        Self::Silence {
            alert_id: alert_id.to_owned(),
            incident_generation,
            reason: reason.trim().to_owned(),
            silence_seconds: hours.saturating_mul(3_600),
        }
    }

    fn alert_id(&self) -> &str {
        match self {
            Self::Acknowledge { alert_id, .. } | Self::Silence { alert_id, .. } => alert_id,
        }
    }

    fn reason(&self) -> &str {
        match self {
            Self::Acknowledge { reason, .. } | Self::Silence { reason, .. } => reason,
        }
    }
}

/// 未决宿主的实体键；同一告警的确认与静默互斥。
fn alert_command_entity(alert_id: &str) -> String {
    format!("alert:{alert_id}")
}

/// 服务端明确拒绝、可能意味着列表快照过时（signal 已恢复或状态已变）的告警命令错误码；
/// invalid_alert_command 也可能来自参数校验，此时重读同样无害。其余失败（含结果未知）不触发重读。
fn alert_snapshot_stale(code: &str) -> bool {
    matches!(code, "invalid_alert_command" | "alert_stale")
}

fn submit_alert_command(
    pending: PendingCommands,
    mut notice: Signal<Option<AdminActionFeedback>>,
    refresh: Signal<u64>,
    command: AlertCommand,
) {
    let action = match command {
        AlertCommand::Acknowledge { .. } => t!("alert-console-acknowledge"),
        AlertCommand::Silence { .. } => t!("alert-console-silence"),
    };
    let submitted = pending.submit(
        alert_command_entity(command.alert_id()),
        action,
        move |command_id| {
            let command = command.clone();
            async move {
                let result = match command {
                    AlertCommand::Acknowledge { alert_id, incident_generation, reason } => {
                        acknowledge_admin_alert(command_id, &alert_id, incident_generation, &reason)
                            .await
                    }
                    AlertCommand::Silence {
                        alert_id,
                        incident_generation,
                        reason,
                        silence_seconds,
                    } => {
                        silence_admin_alert(
                            command_id,
                            &alert_id,
                            incident_generation,
                            silence_seconds,
                            &reason,
                        )
                        .await
                    }
                };
                result.map(|_| ()).map_err(|failure| failure.code)
            }
        },
        move |()| set_if_alive(notice, Some(AdminActionFeedback::Success)),
        move |code| {
            // 这两类明确拒绝说明列表快照已过时（signal 已恢复或状态已变化），且拒绝本身未写入任何
            // 事实：重读列表供操作员判断下一步。结果未知等其他失败不重读，只能原样重试。
            if alert_snapshot_stale(&code) {
                let mut refresh = refresh;
                if let Ok(mut generation) = refresh.try_write() {
                    *generation = generation.saturating_add(1);
                }
            }
            set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
        },
    );
    if submitted {
        notice.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::{AlertCommand, SILENCE_HOUR_OPTIONS, alert_command_entity, alert_snapshot_stale};

    #[test]
    fn frozen_commands_keep_row_generation_trim_reason_and_convert_duration() {
        assert_eq!(
            AlertCommand::acknowledge("ab", 1, "  disk replaced \n"),
            AlertCommand::Acknowledge {
                alert_id: "ab".to_owned(),
                incident_generation: 1,
                reason: "disk replaced".to_owned()
            }
        );
        assert_eq!(
            AlertCommand::silence("ab", 2, 8, " maintenance "),
            AlertCommand::Silence {
                alert_id: "ab".to_owned(),
                incident_generation: 2,
                reason: "maintenance".to_owned(),
                silence_seconds: 28_800,
            }
        );
        assert_eq!(
            alert_command_entity(AlertCommand::silence("ab", 2, 1, "").alert_id()),
            "alert:ab"
        );
    }

    #[test]
    fn every_row_duration_stays_within_server_silence_bounds() {
        for hours in SILENCE_HOUR_OPTIONS {
            let AlertCommand::Silence { silence_seconds, .. } =
                AlertCommand::silence("ab", 2, hours, "")
            else {
                panic!("silence command expected");
            };
            assert!((60..=7 * 24 * 3_600).contains(&silence_seconds));
        }
    }

    #[test]
    fn only_definite_stale_rejections_reload_the_alert_list() {
        assert!(alert_snapshot_stale("invalid_alert_command"));
        assert!(alert_snapshot_stale("alert_stale"));
        // 结果未知、冲突与网络失败不重读：未决宿主保留原命令，只允许原样重试。
        for code in ["outcome_unknown", "command_conflict", "network_error", "http_503"] {
            assert!(!alert_snapshot_stale(code), "{code} 不应触发重读");
        }
    }
}
