//! 独立于 runtime 文件配置能力的运行时开关卡片：全部开关只读展示；W1.9 外部访问暂停可在
//! 此两阶段确认后写入，其余开关仍只能经 CLI 修改。

use crate::api::{
    AdminApiFailure, ExternalAccessPauseAttempt, fetch_runtime_switches, pause_outcome_unknown,
    submit_external_access_pause,
};
use auction_contracts::RuntimeSwitchListResponse;
use auction_web_shared::{AsyncState, AsyncStateKind, legacy_t as t, use_confirm_command};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;

/// 可选暂停时长（小时）；上限与服务端的 7 天一致。
const PAUSE_HOURS: [i64; 6] = [1, 2, 6, 24, 72, 168];

#[component]
pub(super) fn RuntimeSwitchCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let snapshot = use_resource(move || {
        let _ = refresh();
        fetch_runtime_switches()
    });
    let current = snapshot.read_unchecked().clone();
    rsx! {
        section { class: "admin-runtime-switches admin-panel",
            h2 { {t!("admin-switches-title")} }
            p { class: "admin-panel__note", {t!("admin-switches-note")} }
            button { class: "admin-btn-secondary", r#type: "button", onclick: move |_| refresh.set(refresh().saturating_add(1)), {t!("admin-switches-refresh")} }
            match current {
                None => rsx! { AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-switches-loading"), detail: "" } },
                Some(Err(code)) => rsx! { AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" } },
                Some(Ok(data)) => {
                    let (revision, paused_until) = pause_state(&data);
                    rsx! {
                        ExternalAccessPauseControl { revision, paused_until, refresh }
                        p { {t!("admin-switches-age")} ": " {data.admin_last_success_age_seconds.map(|age| format!("{age:.1} s")).unwrap_or_else(|| t!("admin-switches-unconfirmed"))} }
                        if data.admin_last_success_age_seconds.is_none_or(|age| age > 10.0) {
                            p { class: "admin-panel__note", {t!("admin-switches-stale")} }
                        }
                        for intent in data.database_intent {
                            article { class: "admin-runtime-switches__entry",
                                h3 { "{intent.name}" }
                                p { {t!("admin-switches-intent")} ": " {intent.operation.clone().unwrap_or_else(|| t!("admin-switches-unset"))} " / " {intent.value.map(|value| value.to_string()).unwrap_or_else(|| "—".to_owned())} " / r{intent.revision}" }
                                p { {t!("admin-switches-expiry")} ": " {intent.expires_at.map(|until| until.to_rfc3339()).unwrap_or_else(|| "—".to_owned())} }
                                p { {t!("admin-switches-actor")} ": " {intent.actor_id.unwrap_or_else(|| "—".to_owned())} " / " {intent.entrypoint.unwrap_or_else(|| "—".to_owned())} }
                                p { {t!("admin-switches-reason")} ": " {intent.reason.unwrap_or_else(|| "—".to_owned())} }
                                for applied in data.admin_applied.iter().filter(|value| value.name == intent.name) {
                                    p { {t!("admin-switches-applied")} ": " {applied.effective_value.to_string()} " / r{applied.revision} / " {t!("admin-switches-baseline")} ": " {applied.toml_baseline.to_string()} }
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

/// 从数据库最新意图读取暂停版本与仍有效的期限；期限以数据库读取时刻判断，不用浏览器时钟。
fn pause_state(data: &RuntimeSwitchListResponse) -> (i64, Option<DateTime<Utc>>) {
    let Some(intent) =
        data.database_intent.iter().find(|item| item.name == "external_access_paused")
    else {
        return (0, None);
    };
    let paused = intent.operation.as_deref() == Some("set")
        && intent.value.as_ref().and_then(serde_json::Value::as_bool) == Some(true);
    let until = intent.expires_at.filter(|until| paused && *until > data.database_time);
    (intent.revision, until)
}

/// 首次冻结命令时计算的绝对期限（整秒，满足服务端微秒精度要求）；重试复用同一值。
fn pause_expiry(hours: i64) -> Option<DateTime<Utc>> {
    let seconds = hours.checked_mul(3_600)?;
    DateTime::from_timestamp(Utc::now().timestamp().checked_add(seconds)?, 0)
}

#[component]
fn ExternalAccessPauseControl(
    revision: i64,
    paused_until: Option<DateTime<Utc>>,
    refresh: Signal<u64>,
) -> Element {
    let mut command = use_confirm_command::<ExternalAccessPauseAttempt, AdminApiFailure>();
    let mut hours = use_signal(|| 2_i64);
    let mut reason = use_signal(String::new);
    let mut accepted = use_signal(|| false);
    let paused = paused_until.is_some();
    let submitting = (command.submitting)();
    let uncertain = (command.uncertain)();
    let confirmation = (command.confirmation)();
    let failure = (command.failure)();

    let begin = move |_| {
        let reason_text = reason().trim().to_owned();
        if reason_text.is_empty() {
            return;
        }
        let selected = hours();
        let expires_at = if paused { None } else { pause_expiry(selected) };
        if !paused && expires_at.is_none() {
            return;
        }
        accepted.set(false);
        // 签名覆盖全部意图字段：同一输入的重试复用 command_id，输入变化即生成新命令。
        let signature = format!("{paused}:{selected}:{revision}:{reason_text}");
        command.begin(signature, |command_id| ExternalAccessPauseAttempt {
            command_id,
            expected_revision: revision,
            paused: !paused,
            expires_at,
            reason: reason_text,
        });
    };
    let mut submit = move |pending: ExternalAccessPauseAttempt| {
        command.submit(
            pending,
            |attempt| async move { submit_external_access_pause(&attempt).await },
            |failure: &AdminApiFailure| pause_outcome_unknown(failure),
            move |_| {
                accepted.set(true);
                reason.set(String::new());
                refresh.set(refresh().saturating_add(1));
            },
            || {},
        );
    };

    rsx! {
        article { class: "admin-runtime-switches__entry",
            h3 { {t!("admin-pause-title")} }
            p { class: "admin-panel__note", {t!("admin-pause-note")} }
            p {
                if let Some(until) = paused_until {
                    {t!("admin-pause-active")} ": " {until.to_rfc3339()}
                } else {
                    {t!("admin-pause-inactive")}
                }
            }
            if accepted() {
                p { class: "admin-shell__banner", role: "status", {t!("admin-pause-done")} }
            }
            if let Some(failure) = failure {
                p { class: "admin-shell__banner", role: "alert", {super::super::admin_api_error_message(&failure.code)} }
            }
            if uncertain {
                if let Some(pending) = (command.command)() {
                    p { class: "admin-panel__note", role: "status", {t!("admin-pause-uncertain")} }
                    button {
                        class: "admin-btn-primary",
                        r#type: "button",
                        disabled: submitting,
                        onclick: move |_| submit(pending.clone()),
                        {t!("admin-pause-retry")}
                    }
                }
            } else if let Some(pending) = confirmation {
                p {
                    if pending.paused {
                        {t!("admin-pause-confirm-pause")} ": " {pending.expires_at.map(|until| until.to_rfc3339()).unwrap_or_default()}
                    } else {
                        {t!("admin-pause-confirm-resume")}
                    }
                }
                button {
                    class: if pending.paused { "admin-btn-danger" } else { "admin-btn-primary" },
                    r#type: "button",
                    disabled: submitting,
                    onclick: move |_| submit(pending.clone()),
                    {t!("admin-pause-confirm")}
                }
                button {
                    class: "admin-btn-secondary",
                    r#type: "button",
                    disabled: submitting,
                    onclick: move |_| command.cancel_confirmation(),
                    {t!("admin-pause-cancel")}
                }
            } else {
                if !paused {
                    label {
                        {t!("admin-pause-duration")}
                        select {
                            value: "{hours()}",
                            onchange: move |event| {
                                let value = event.value().parse::<i64>().ok().filter(|value| PAUSE_HOURS.contains(value));
                                hours.set(value.unwrap_or(2));
                            },
                            for option_hours in PAUSE_HOURS {
                                option { value: "{option_hours}", "{option_hours} h" }
                            }
                        }
                    }
                }
                label {
                    {t!("admin-pause-reason")}
                    input {
                        r#type: "text",
                        maxlength: "1024",
                        value: reason(),
                        oninput: move |event| {
                            let value = event.value();
                            if value.chars().count() <= 1024 && !value.contains('\n') {
                                reason.set(value);
                            }
                        },
                    }
                }
                button {
                    class: if paused { "admin-btn-primary" } else { "admin-btn-danger" },
                    r#type: "button",
                    disabled: submitting || reason().trim().is_empty(),
                    onclick: begin,
                    if paused { {t!("admin-pause-resume")} } else { {t!("admin-pause-start")} }
                }
            }
        }
    }
}
