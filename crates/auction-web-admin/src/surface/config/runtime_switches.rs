//! 独立于 runtime 文件配置能力的运行时开关卡片：全部开关只读展示；外部访问暂停可在
//! 此两阶段确认后写入，其余开关仍只能经 CLI 修改。

use auction_contracts::RuntimeSwitchListResponse;
use auction_time::Timestamp;
use auction_web_shared::{AsyncState, AsyncStateKind, ConfirmCommand, t, use_confirm_command};
#[cfg(target_arch = "wasm32")]
use auction_web_shared::{PageVisibility, timer::TimeoutFuture};
use dioxus::prelude::*;

use super::field_announcement::{FieldLiveRegion, use_field_announcer};
use crate::api::{
    AdminApiFailure, ExternalAccessPauseAttempt, fetch_runtime_switches,
    submit_external_access_pause,
};

/// 可选暂停时长（小时）；上限与服务端的 7 天一致。
const PAUSE_HOURS: [i64; 6] = [1, 2, 6, 24, 72, 168];

/// 暂停命令与草稿由卡片持有，读取失败导致控件卸载时仍保留原样重试所需的 ID 与参数。
/// `PartialEq` 比较 Signal 身份，避免 props 比较订阅控件状态。
#[derive(Clone, Copy)]
struct PauseControlState {
    command: ConfirmCommand<ExternalAccessPauseAttempt, AdminApiFailure>,
    hours: Signal<i64>,
    reason: Signal<String>,
    accepted: Signal<bool>,
    required_generation: Signal<u64>,
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
            && self.required_generation == other.required_generation
    }
}

impl PauseControlState {
    /// 提交或原样重试；结果未知时 `ConfirmCommand` 只重放冻结的原命令。
    fn submit(mut self, pending: ExternalAccessPauseAttempt, mut refresh: Signal<u64>) {
        // 冲突清除原命令后，迟到的按钮事件不能再发送它。
        if self.command.command.peek().as_ref() != Some(&pending) {
            return;
        }
        let mut accepted = self.accepted;
        let mut reason = self.reason;
        let mut command = self.command;
        let mut required_generation = self.required_generation;
        self.command.submit(
            pending,
            |attempt| async move {
                #[cfg(test)]
                if let Some(requests) = try_consume_context::<std::rc::Rc<tests::PauseRequests>>() {
                    return requests.write(attempt);
                }
                submit_external_access_pause(&attempt).await
            },
            AdminApiFailure::outcome_unknown,
            move |_| {
                accepted.set(true);
                reason.set(String::new());
                refresh.set(refresh().saturating_add(1));
            },
            move || {
                let rejection = command
                    .failure
                    .peek()
                    .as_ref()
                    .filter(|failure| failure.resolves_prior_attempt())
                    .cloned();
                if let Some(failure) = rejection {
                    // 账本已核对才能丢弃旧 ID。重新读取版本后，由操作员确认新命令。
                    let generation = refresh.peek().saturating_add(1);
                    required_generation.set(generation);
                    command.reset();
                    command.failure.set(Some(failure));
                    refresh.set(generation);
                }
            },
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
        required_generation: use_signal(|| 0),
    };
    let pause_uncertain = (pause.command.uncertain)();
    let snapshot = use_resource(move || {
        let generation = refresh();
        async move {
            #[cfg(test)]
            if let Some(requests) = try_consume_context::<std::rc::Rc<tests::PauseRequests>>() {
                return (generation, requests.read().await);
            }
            (generation, fetch_runtime_switches().await)
        }
    });
    #[cfg(target_arch = "wasm32")]
    {
        let visibility = use_hook(PageVisibility::new);
        use_effect({
            let visibility = visibility.clone();
            move || visibility.install(move || refresh.set(refresh().saturating_add(1)))
        });
        use_drop(move || visibility.cleanup());
        use_resource(move || {
            let generation = refresh();
            let delay = snapshot
                .read()
                .as_ref()
                .filter(|(loaded, _)| *loaded == generation)
                .and_then(|(_, result)| result.as_ref().ok())
                .and_then(pause_refresh_millis);
            async move {
                let Some(mut remaining) = delay else { return };
                // CLI 可写入较长的期限，需分片等待以免超出浏览器定时器上限。
                while remaining > 0 {
                    let slice = remaining.min(i64::from(i32::MAX)) as u32;
                    TimeoutFuture::new(slice).await;
                    remaining -= i64::from(slice);
                }
                // 定时器仅唤醒读取，由数据库快照确认到期。复核已开始时旧任务不得再次刷新。
                if *refresh.peek() == generation {
                    refresh.set(generation.saturating_add(1));
                }
            }
        });
    }
    let current = snapshot.read_unchecked().as_ref().cloned();
    rsx! {
        section { class: "admin-runtime-switches admin-panel",
            h2 { {t!("admin-switches-title")} }
            p { class: "admin-panel__note", {t!("admin-switches-note")} }
            button { class: "admin-btn-secondary", r#type: "button", onclick: move |_| refresh.set(refresh().saturating_add(1)), {t!("admin-switches-refresh")} }
            if let Some(failure) = (pause.command.failure)() {
                p { class: "admin-shell__banner", role: "alert", {super::super::admin_api_error_message(&failure.code)} }
            }
            match current {
                // 快照不可用时仍保留结果未知命令的原样重试入口。
                None => rsx! {
                    if pause_uncertain { PendingPauseRetry { state: pause, refresh } }
                    AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-switches-loading"), detail: "" }
                },
                Some((_, Err(code))) => rsx! {
                    if pause_uncertain { PendingPauseRetry { state: pause, refresh } }
                    AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" }
                },
                Some((loaded_generation, Ok(data))) => {
                    let (revision, paused, paused_until) = pause_state(&data);
                    rsx! {
                        ExternalAccessPauseControl { state: pause, revision, paused, paused_until, refresh, loaded_generation }
                        p { {t!("admin-switches-age")} ": " {data.admin_last_success_age_seconds.map_or_else(|| t!("admin-switches-unconfirmed"), |age| format!("{age:.1} s"))} }
                        if data.admin_last_success_age_seconds.is_none_or(|age| age > 10.0) {
                            p { class: "admin-panel__note", {t!("admin-switches-stale")} }
                        }
                        for intent in data.database_intent {
                            article { class: "admin-runtime-switches__entry",
                                h3 { "{intent.name}" }
                                p { {t!("admin-switches-intent")} ": " {intent.operation.clone().unwrap_or_else(|| t!("admin-switches-unset"))} " / " {intent.value.map_or_else(|| "—".to_owned(), |value| value.to_string())} " / r{intent.revision}" }
                                p { {t!("admin-switches-expiry")} ": " {intent.expires_at.map_or_else(|| "—".to_owned(), auction_time::Timestamp::to_rfc3339)} }
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
fn pause_state(data: &RuntimeSwitchListResponse) -> (i64, bool, Option<Timestamp>) {
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

#[cfg(any(test, target_arch = "wasm32"))]
fn pause_refresh_millis(data: &RuntimeSwitchListResponse) -> Option<i64> {
    let remaining = pause_state(data).2? - data.database_time;
    // 亚毫秒余量向上取整，避免在数据库期限之前补读。
    Some(
        remaining
            .num_milliseconds()
            .saturating_add(i64::from(remaining.subsec_nanos() % 1_000_000 != 0)),
    )
}

/// 首次冻结命令时计算的绝对期限（整秒，满足服务端微秒精度要求）；重试复用同一值。
fn pause_expiry(hours: i64) -> Option<Timestamp> {
    let seconds = hours.checked_mul(3_600)?;
    Timestamp::from_timestamp(Timestamp::now().timestamp().checked_add(seconds)?, 0)
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

const PAUSE_REASON_ID: &str = "admin-pause-reason";

fn reason_error_text() -> String {
    t!(
        "admin-pause-reason-invalid",
        maximum: ExternalAccessPauseAttempt::MAX_REASON_BYTES as i64
    )
}

#[component]
fn ExternalAccessPauseControl(
    state: PauseControlState,
    revision: i64,
    paused: bool,
    paused_until: Option<Timestamp>,
    refresh: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let PauseControlState { mut command, mut hours, mut reason, mut accepted, required_generation } =
        state;
    let submitting = (command.submitting)();
    let uncertain = (command.uncertain)();
    let confirmation = (command.confirmation)();
    let may_prepare = !uncertain && confirmation.is_none() && !submitting;
    let waiting_for_snapshot = loaded_generation < required_generation();
    let mut reason_edited = use_signal(|| false);
    let mut reason_invalid = use_signal(|| false);
    let announcer = use_field_announcer();

    let begin = move |event: FormEvent| {
        event.prevent_default();
        if !may_prepare || loaded_generation < *required_generation.peek() {
            return;
        }
        let reason_text = reason().trim().to_owned();
        if !ExternalAccessPauseAttempt::valid_reason(&reason_text) {
            reason_invalid.set(true);
            announcer.invalid_submit(PAUSE_REASON_ID, reason_error_text(), true);
            return;
        }
        reason_invalid.set(false);
        announcer.clear();
        reason_edited.set(false);
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
        form {
            class: "admin-runtime-switches__entry admin-runtime-pause",
            method: "post",
            action: "/api/v1/admin/runtime-switches/external-access-paused",
            aria_labelledby: "admin-runtime-pause-title",
            novalidate: true,
            onsubmit: begin,
            h3 { id: "admin-runtime-pause-title", {t!("admin-pause-title")} }
            p { class: "admin-panel__note", {t!("admin-pause-note")} }
            p {
                if paused {
                    {t!("admin-pause-active")} ": " {paused_until.map_or_else(|| t!("admin-pause-no-expiry"), auction_time::Timestamp::to_rfc3339)}
                } else {
                    {t!("admin-pause-inactive")}
                }
            }
            if accepted() {
                p { class: "admin-shell__banner", role: "status", {t!("admin-pause-done")} }
            }
            if uncertain {
                PendingPauseRetry { state, refresh }
            } else if let Some(pending) = confirmation {
                p {
                    if pending.paused {
                        {t!("admin-pause-confirm-pause")} ": " {pending.expires_at.map(auction_time::Timestamp::to_rfc3339).unwrap_or_default()}
                    } else {
                        {t!("admin-pause-confirm-resume")}
                    }
                }
                button {
                    class: if pending.paused { "admin-btn-danger" } else { "admin-btn-primary" },
                    r#type: "button",
                    disabled: submitting || waiting_for_snapshot,
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
                            name: "duration",
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
                div {
                    label {
                        r#for: PAUSE_REASON_ID,
                        {t!("admin-pause-reason", maximum: ExternalAccessPauseAttempt::MAX_REASON_BYTES as i64)}
                        input {
                            id: PAUSE_REASON_ID,
                            name: "reason",
                            r#type: "text",
                            required: true,
                            aria_invalid: reason_invalid().to_string(),
                            aria_describedby: reason_invalid().then_some("admin-pause-reason-error"),
                            value: reason(),
                            oninput: move |event| {
                                let value = event.value();
                                if ExternalAccessPauseAttempt::valid_reason(value.trim()) {
                                    reason_invalid.set(false);
                                    announcer.clear_field(PAUSE_REASON_ID);
                                }
                                reason_edited.set(true);
                                reason.set(value);
                            },
                            onblur: move |_| {
                                if reason_edited() && !reason_invalid()
                                    && !ExternalAccessPauseAttempt::valid_reason(reason().trim())
                                {
                                    reason_invalid.set(true);
                                    announcer.announce_on_blur(PAUSE_REASON_ID, reason_error_text());
                                }
                            },
                        }
                    }
                    if reason_invalid() {
                        p { id: "admin-pause-reason-error", class: "admin-field-error", {reason_error_text()} }
                    }
                }
                button {
                    class: if paused { "admin-btn-primary" } else { "admin-btn-danger" },
                    r#type: "submit",
                    disabled: submitting || waiting_for_snapshot,
                    if paused { {t!("admin-pause-resume")} } else { {t!("admin-pause-start")} }
                }
            }
        }
        FieldLiveRegion { announcer }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use auction_contracts::{RuntimeSwitchIntentDto, RuntimeSwitchListResponse};
    use auction_time::{TimeDelta, Timestamp};
    use auction_web_shared::TransportError;
    use dioxus::core::ElementId;
    use dioxus::prelude::*;
    use serde_json::json;

    use super::{
        PAUSE_REASON_ID, RuntimeSwitchCard, pause_refresh_millis, pause_state, reason_error_text,
    };
    use crate::api::{AdminApiFailure, ExternalAccessPauseAttempt};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, assert_field, button, click_button, has_live_text, mount_writer, settle_writer,
        writer_form,
    };
    use crate::surface::realtime::{TestPage, text_attributes};

    pub(super) struct PauseRequests {
        reads: Cell<usize>,
        refresh_reply: RecordRead<RuntimeSwitchListResponse>,
        writes: RefCell<Vec<ExternalAccessPauseAttempt>>,
    }

    impl PauseRequests {
        fn new() -> Self {
            Self {
                reads: Cell::new(0),
                refresh_reply: RecordRead::new(),
                writes: RefCell::new(Vec::new()),
            }
        }

        pub(super) async fn read(&self) -> Result<RuntimeSwitchListResponse, String> {
            let prior = self.reads.replace(self.reads.get() + 1);
            if prior == 0 {
                return Ok(snapshot("clear", None, Timestamp::now()));
            }
            self.refresh_reply.fetch().await
        }

        fn finish_refresh(&self) {
            let mut data = snapshot("set", None, Timestamp::now());
            data.database_intent[0].revision = 8;
            self.refresh_reply.respond(Ok(data));
        }

        pub(super) fn write(
            &self,
            attempt: ExternalAccessPauseAttempt,
        ) -> Result<RuntimeSwitchIntentDto, AdminApiFailure> {
            let mut writes = self.writes.borrow_mut();
            writes.push(attempt);
            let failure = match writes.len() {
                1 => AdminApiFailure::client(TransportError::Timeout.code()),
                2 | 4 => AdminApiFailure::from_http_body(
                    429,
                    r#"{"code":"rate_limited","message":"请求过多","data":null,"request_id":null}"#,
                ),
                3 => AdminApiFailure::from_http_body(
                    409,
                    r#"{"code":"runtime_switch_conflict","message":"版本已变化","data":null,"request_id":null}"#,
                ),
                _ => panic!("收到额外的暂停命令"),
            };
            Err(failure)
        }
    }

    fn mount_card(requests: Rc<PauseRequests>) -> (TestPage, ElementId, ElementId) {
        let (page, edits) = mount_writer(move || {
            use_context_provider(|| requests.clone());
            rsx! { RuntimeSwitchCard {} }
        });
        let reason = text_attributes(&edits, "id")
            .find_map(|(id, value)| (value == PAUSE_REASON_ID).then_some(id))
            .unwrap();
        (page, writer_form(&edits), reason)
    }

    #[test]
    fn ordinary_refresh_keeps_the_form_draft_and_validation_until_reply() {
        let requests = Rc::new(PauseRequests::new());
        let (mut page, form, reason) = mount_card(requests.clone());
        let draft = "界".repeat(ExternalAccessPauseAttempt::MAX_REASON_BYTES / 3 + 1);
        page.dispatch("input", reason, &draft);
        page.dispatch("blur", reason, "");
        let error = page.dom().in_scope(ScopeId::APP, reason_error_text);
        assert!(has_live_text(&page, &error));

        click_button(&mut page, "刷新");
        assert_eq!(requests.reads.get(), 2);
        assert!(has_live_text(&page, "暂停外部访问…"));
        assert_field(&page, reason, &draft, false);
        assert!(has_live_text(&page, &error));
        page.dispatch("input", reason, "演练");
        page.dispatch("submit", form, "");
        assert!(button(&page, "确认").is_some_and(|(_, disabled)| !disabled));
        click_button(&mut page, "返回");

        requests.finish_refresh();
        settle_writer(&mut page);
        assert!(has_live_text(&page, "恢复外部访问…"));
    }

    #[test]
    fn pause_retries_verbatim_until_a_ledger_checked_conflict_then_rereads() {
        let requests = Rc::new(PauseRequests::new());
        let (mut page, form, reason) = mount_card(requests.clone());
        page.dispatch("input", reason, "演练");
        page.dispatch("submit", form, "");
        click_button(&mut page, "确认");
        assert!(has_live_text(&page, "提交结果未知：请原样重试，不要新建命令。"));
        click_button(&mut page, "原样重试");
        assert!(button(&page, "原样重试").is_some());
        assert_eq!(requests.reads.get(), 1);

        click_button(&mut page, "原样重试");
        {
            let writes = requests.writes.borrow();
            assert_eq!(writes.len(), 3);
            assert_eq!(writes[0], writes[1]);
            assert_eq!(writes[1], writes[2]);
        }
        assert!(button(&page, "原样重试").is_none());
        assert!(has_live_text(&page, "数据已变化，或本次命令与已有请求冲突。请重新加载后再试。"));
        assert_eq!(requests.reads.get(), 2);
        assert!(has_live_text(&page, "暂停外部访问…"));
        page.dispatch("submit", form, "");
        assert!(button(&page, "确认").is_none());
        assert_eq!(requests.writes.borrow().len(), 3);

        requests.finish_refresh();
        settle_writer(&mut page);
        assert!(has_live_text(&page, "恢复外部访问…"));
        page.dispatch("submit", form, "");
        click_button(&mut page, "确认");
        let writes = requests.writes.borrow();
        assert_eq!(writes.len(), 4);
        assert_eq!(writes[3].expected_revision, 8);
        assert_ne!(writes[3].command_id, writes[0].command_id);
    }

    fn snapshot(
        operation: &str,
        expires_at: Option<Timestamp>,
        now: Timestamp,
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
        let now = Timestamp::now();
        assert_eq!(pause_state(&snapshot("set", None, now)), (7, true, None));
    }

    #[test]
    fn timed_pause_is_active_until_database_time_reaches_expiry() {
        let now = Timestamp::now();
        let until = now + TimeDelta::hours(2);
        assert_eq!(pause_state(&snapshot("set", Some(until), now)), (7, true, Some(until)));
        let expired = now - TimeDelta::seconds(1);
        assert_eq!(pause_state(&snapshot("set", Some(expired), now)), (7, false, None));
    }

    #[test]
    fn cleared_or_missing_switch_is_not_paused() {
        let now = Timestamp::now();
        assert_eq!(pause_state(&snapshot("clear", None, now)), (7, false, None));
        let empty = RuntimeSwitchListResponse {
            database_time: now,
            database_intent: Vec::new(),
            admin_last_success_age_seconds: None,
            admin_applied: Vec::new(),
        };
        assert_eq!(pause_state(&empty), (0, false, None));
    }

    #[test]
    fn timed_pause_refresh_uses_database_time_and_rounds_up() {
        let database_time = Timestamp::from_timestamp(1_700_000_000, 0).unwrap();
        let until = database_time + TimeDelta::milliseconds(10) + TimeDelta::microseconds(300);
        let before_expiry = snapshot("set", Some(until), database_time);
        assert_eq!(pause_refresh_millis(&before_expiry), Some(11));
        assert_eq!(pause_refresh_millis(&snapshot("set", Some(until), until)), None);
    }

    #[test]
    fn indefinite_or_cleared_pause_never_schedules_expiry_refresh() {
        let now = Timestamp::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(pause_refresh_millis(&snapshot("set", None, now)), None);
        assert_eq!(pause_refresh_millis(&snapshot("clear", None, now)), None);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod form_tests {
    use std::rc::Rc;

    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use auction_web_shared::use_confirm_command;
    use dioxus::core::{AttributeValue, ElementId, Mutation, ScopeId};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        ExternalAccessPauseControl, PAUSE_REASON_ID, PauseControlState, reason_error_text,
    };
    use crate::api::ExternalAccessPauseAttempt;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::test_form::{FormBrowser, dispatch, listener};

    fn pause_form() -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        let state = PauseControlState {
            command: use_confirm_command(),
            hours: use_signal(|| 2),
            reason: use_signal(|| {
                "界".repeat(ExternalAccessPauseAttempt::MAX_REASON_BYTES / 3 + 1)
            }),
            accepted: use_signal(|| false),
            required_generation: use_signal(|| 0),
        };
        let refresh = use_signal(|| 0);
        rsx! { ExternalAccessPauseControl { state, revision: 7, paused: false, paused_until: None, refresh, loaded_generation: 0 } }
    }

    fn mount(focused: &'static str) -> (VirtualDom, Rc<FormBrowser>, ElementId, String) {
        let browser = Rc::new(FormBrowser::default());
        browser.focused.set(focused);
        let mut dom = VirtualDom::new(pause_form).with_root_context(browser.clone());
        let submit = listener(&dom.rebuild_to_vec().edits, "submit");
        let error = dom.in_scope(ScopeId::APP, reason_error_text);
        (dom, browser, submit, error)
    }

    fn updated_text(edits: &[Mutation], text: &str) -> Option<ElementId> {
        edits.iter().find_map(|edit| match edit {
            Mutation::SetText { value, id } if value == text => Some(*id),
            _ => None,
        })
    }

    #[test]
    fn enter_with_focus_on_invalid_reason_announces_without_moving_focus() {
        let (mut dom, browser, submit, error) = mount(PAUSE_REASON_ID);
        dispatch(&dom, submit, "submit", "");
        let rendered = dom.render_immediate_to_vec();
        assert!(rendered.edits.iter().any(|edit| matches!(edit,
            Mutation::SetAttribute { name: "aria-invalid", value: AttributeValue::Text(value), .. }
                if value == "true"
        )));
        assert_eq!(updated_text(&rendered.edits, &error), None);
        assert_eq!(browser.moved_focus_to.get(), None);

        browser.fire_timer();
        assert!(updated_text(&dom.render_immediate_to_vec().edits, &error).is_some());
    }

    #[test]
    fn submit_from_elsewhere_moves_focus_to_invalid_reason() {
        let (mut dom, browser, submit, error) = mount("pause-submit");
        dispatch(&dom, submit, "submit", "");
        assert_eq!(browser.moved_focus_to.get(), Some(PAUSE_REASON_ID));
        assert_eq!(updated_text(&dom.render_immediate_to_vec().edits, &error), None);
    }

    #[test]
    fn repeat_submit_clears_the_live_region_before_announcing_the_same_error() {
        let (mut dom, browser, submit, error) = mount(PAUSE_REASON_ID);
        dispatch(&dom, submit, "submit", "");
        dom.render_immediate_to_vec();
        browser.fire_timer();
        let region = updated_text(&dom.render_immediate_to_vec().edits, &error).unwrap();

        dispatch(&dom, submit, "submit", "");
        assert_eq!(updated_text(&dom.render_immediate_to_vec().edits, ""), Some(region));
        browser.fire_timer();
        assert_eq!(updated_text(&dom.render_immediate_to_vec().edits, &error), Some(region));
        assert_eq!(browser.moved_focus_to.get(), None);
    }
}
