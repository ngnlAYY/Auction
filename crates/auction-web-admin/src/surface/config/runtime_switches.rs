//! 独立于 runtime 文件配置能力的运行时开关卡片：全部开关只读展示；外部访问暂停可在
//! 此两阶段确认后写入，其余开关仍只能经 CLI 修改。

use crate::api::{
    AdminApiFailure, ExternalAccessPauseAttempt, fetch_runtime_switches, pause_outcome_unknown,
    submit_external_access_pause,
};
use auction_contracts::RuntimeSwitchListResponse;
use auction_web_shared::{AsyncState, AsyncStateKind, ConfirmCommand, t, use_confirm_command};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;

/// 可选暂停时长（小时）；上限与服务端的 7 天一致。
const PAUSE_HOURS: [i64; 6] = [1, 2, 6, 24, 72, 168];

/// 暂停控件的全部本地状态由卡片持有。快照加载中或读取失败时控件会卸载；状态若留在控件内，
/// 结果未知的命令 ID 与冻结参数随之丢失，重新挂载后只能发新命令，违反“结果未知只允许原样
/// 重试”。包一层新类型只为实现 `PartialEq`（`#[component]` props 要求）：逐字段比较 Signal
/// 身份，不读值、不触发订阅。
#[derive(Clone, Copy)]
struct PauseControlState {
    command: ConfirmCommand<ExternalAccessPauseAttempt, AdminApiFailure>,
    hours: Signal<i64>,
    reason: Signal<String>,
    accepted: Signal<bool>,
}

impl PartialEq for PauseControlState {
    fn eq(&self, other: &Self) -> bool {
        self.command.command == other.command.command
            && self.command.confirmation == other.command.confirmation
            && self.command.submitting == other.command.submitting
            && self.command.failure == other.command.failure
            && self.command.uncertain == other.command.uncertain
            && self.hours == other.hours
            && self.reason == other.reason
            && self.accepted == other.accepted
    }
}

impl PauseControlState {
    /// 提交或原样重试；结果未知时 `ConfirmCommand` 只重放冻结的原命令。
    fn submit(mut self, pending: ExternalAccessPauseAttempt, mut refresh: Signal<u64>) {
        let mut accepted = self.accepted;
        let mut reason = self.reason;
        self.command.submit(
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
    }
}

#[component]
pub(super) fn RuntimeSwitchCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let pause = PauseControlState {
        command: use_confirm_command(),
        hours: use_signal(|| 2_i64),
        reason: use_signal(String::new),
        accepted: use_signal(|| false),
    };
    let pause_uncertain = (pause.command.uncertain)();
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
                // 快照不可用时仍保留结果未知命令的原样重试入口。
                None => rsx! {
                    if pause_uncertain { PendingPauseRetry { state: pause, refresh } }
                    AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-switches-loading"), detail: "" }
                },
                Some(Err(code)) => rsx! {
                    if pause_uncertain { PendingPauseRetry { state: pause, refresh } }
                    AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" }
                },
                Some(Ok(data)) => {
                    let (revision, paused, paused_until) = pause_state(&data);
                    rsx! {
                        ExternalAccessPauseControl { state: pause, revision, paused, paused_until, refresh }
                        p { {t!("admin-switches-age")} ": " {data.admin_last_success_age_seconds.map_or_else(|| t!("admin-switches-unconfirmed"), |age| format!("{age:.1} s"))} }
                        if data.admin_last_success_age_seconds.is_none_or(|age| age > 10.0) {
                            p { class: "admin-panel__note", {t!("admin-switches-stale")} }
                        }
                        for intent in data.database_intent {
                            article { class: "admin-runtime-switches__entry",
                                h3 { "{intent.name}" }
                                p { {t!("admin-switches-intent")} ": " {intent.operation.clone().unwrap_or_else(|| t!("admin-switches-unset"))} " / " {intent.value.map_or_else(|| "—".to_owned(), |value| value.to_string())} " / r{intent.revision}" }
                                p { {t!("admin-switches-expiry")} ": " {intent.expires_at.map_or_else(|| "—".to_owned(), |until| until.to_rfc3339())} }
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

/// 从数据库最新意图读取暂停版本、是否仍暂停与期限；期限以数据库读取时刻判断，不用浏览器时钟。
///
/// 是否暂停与期限分开返回：CLI `--no-expiry` 写入的无期限暂停在 `RuntimeSwitches` 中持续有效，
/// 不能因缺少期限显示为未暂停，否则界面会提供“暂停”并用定时暂停覆盖它，到期后意外恢复外部访问。
fn pause_state(data: &RuntimeSwitchListResponse) -> (i64, bool, Option<DateTime<Utc>>) {
    let Some(intent) =
        data.database_intent.iter().find(|item| item.name == "external_access_paused")
    else {
        return (0, false, None);
    };
    let set = intent.operation.as_deref() == Some("set")
        && intent.value.as_ref().and_then(serde_json::Value::as_bool) == Some(true);
    let paused = set && intent.expires_at.is_none_or(|until| until > data.database_time);
    (intent.revision, paused, intent.expires_at.filter(|_| paused))
}

/// 首次冻结命令时计算的绝对期限（整秒，满足服务端微秒精度要求）；重试复用同一值。
fn pause_expiry(hours: i64) -> Option<DateTime<Utc>> {
    let seconds = hours.checked_mul(3_600)?;
    DateTime::from_timestamp(Utc::now().timestamp().checked_add(seconds)?, 0)
}

/// 结果未知时只显示原命令的重试；参数来自冻结的命令，不读取当前快照。
#[component]
fn PendingPauseRetry(state: PauseControlState, refresh: Signal<u64>) -> Element {
    let submitting = (state.command.submitting)();
    rsx! {
        if let Some(pending) = (state.command.command)() {
            p { class: "admin-panel__note", role: "status", {t!("admin-pause-uncertain")} }
            button {
                class: "admin-btn-primary",
                r#type: "button",
                disabled: submitting,
                onclick: move |_| state.submit(pending.clone(), refresh),
                {t!("admin-pause-retry")}
            }
        }
    }
}

#[component]
fn ExternalAccessPauseControl(
    state: PauseControlState,
    revision: i64,
    paused: bool,
    paused_until: Option<DateTime<Utc>>,
    refresh: Signal<u64>,
) -> Element {
    let PauseControlState { mut command, mut hours, mut reason, mut accepted } = state;
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

    rsx! {
        article { class: "admin-runtime-switches__entry admin-runtime-pause",
            h3 { {t!("admin-pause-title")} }
            p { class: "admin-panel__note", {t!("admin-pause-note")} }
            p {
                if paused {
                    {t!("admin-pause-active")} ": " {paused_until.map_or_else(|| t!("admin-pause-no-expiry"), |until| until.to_rfc3339())}
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
                PendingPauseRetry { state, refresh }
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
                    onclick: move |_| state.submit(pending.clone(), refresh),
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

#[cfg(test)]
mod tests {
    use super::pause_state;
    use auction_contracts::{RuntimeSwitchIntentDto, RuntimeSwitchListResponse};
    use chrono::{DateTime, TimeDelta, Utc};
    use serde_json::json;

    fn snapshot(
        operation: &str,
        expires_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> RuntimeSwitchListResponse {
        RuntimeSwitchListResponse {
            database_time: now,
            database_intent: vec![RuntimeSwitchIntentDto {
                name: "external_access_paused".to_owned(),
                revision: 7,
                operation: Some(operation.to_owned()),
                value: (operation == "set").then_some(json!(true)),
                expires_at,
                actor_id: Some("operator".to_owned()),
                entrypoint: Some("cli".to_owned()),
                reason: Some("drill".to_owned()),
            }],
            admin_last_success_age_seconds: Some(1.0),
            admin_applied: Vec::new(),
        }
    }

    #[test]
    fn indefinite_pause_stays_paused_and_offers_resume() {
        let now = Utc::now();
        assert_eq!(pause_state(&snapshot("set", None, now)), (7, true, None));
    }

    #[test]
    fn timed_pause_is_active_until_database_time_reaches_expiry() {
        let now = Utc::now();
        let until = now + TimeDelta::hours(2);
        assert_eq!(pause_state(&snapshot("set", Some(until), now)), (7, true, Some(until)));
        let expired = now - TimeDelta::seconds(1);
        assert_eq!(pause_state(&snapshot("set", Some(expired), now)), (7, false, None));
    }

    #[test]
    fn cleared_or_missing_switch_is_not_paused() {
        let now = Utc::now();
        assert_eq!(pause_state(&snapshot("clear", None, now)), (7, false, None));
        let empty = RuntimeSwitchListResponse {
            database_time: now,
            database_intent: Vec::new(),
            admin_last_success_age_seconds: None,
            admin_applied: Vec::new(),
        };
        assert_eq!(pause_state(&empty), (0, false, None));
    }
}
