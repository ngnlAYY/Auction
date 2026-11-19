//! 独立于 runtime 文件配置能力的运行时开关卡片：全部开关只读展示；外部访问暂停可在
//! 此两阶段确认后写入，其余开关仍只能经 CLI 修改。

use auction_contracts::RuntimeSwitchListResponse;
use auction_time::Timestamp;
use auction_web_shared::{AsyncState, AsyncStateKind, t};
#[cfg(target_arch = "wasm32")]
use auction_web_shared::{PageVisibility, timer::TimeoutFuture};
use dioxus::prelude::*;

use super::field_announcement::{FieldLiveRegion, use_field_announcer};
use crate::api::{
    ExternalAccessPauseAttempt, fetch_runtime_switches, submit_external_access_pause,
};
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandPanel, PendingCommands, set_if_alive,
    use_pending_commands,
};

/// 可选暂停时长（小时）；上限与服务端的 7 天一致。
const PAUSE_HOURS: [i64; 6] = [1, 2, 6, 24, 72, 168];
const PAUSE_ENTITY: &str = "runtime-switch:external_access_paused";

#[derive(Clone, PartialEq)]
struct PauseConfirmation {
    expected_revision: i64,
    paused: bool,
    expires_at: Option<Timestamp>,
    reason: String,
}

/// 卡片只持有草稿及确认；已提交的暂停或恢复由会话宿主原样重试。
#[derive(Clone, Copy, PartialEq)]
struct PauseControlState {
    pending: PendingCommands,
    confirmation: Signal<Option<PauseConfirmation>>,
    hours: Signal<i64>,
    reason: Signal<String>,
    accepted: Signal<bool>,
}

impl PauseControlState {
    fn submit(mut self, confirmation: PauseConfirmation) {
        if self.confirmation.peek().as_ref() != Some(&confirmation) {
            return;
        }
        let action =
            if confirmation.paused { t!("admin-pause-start") } else { t!("admin-pause-resume") };
        // 暂停和恢复共用实体；原样重试复用确认时冻结的绝对期限与版本。
        if self.pending.submit_with_receipt(
            PAUSE_ENTITY.to_owned(),
            action,
            move |command_id| {
                let confirmation = confirmation.clone();
                async move {
                    submit_external_access_pause(&ExternalAccessPauseAttempt {
                        command_id,
                        expected_revision: confirmation.expected_revision,
                        paused: confirmation.paused,
                        expires_at: confirmation.expires_at,
                        reason: confirmation.reason,
                    })
                    .await
                    .map_err(|failure| failure.code)
                }
            },
            move |_, source_alive| {
                if source_alive {
                    set_if_alive(self.accepted, true);
                    set_if_alive(self.reason, String::new());
                }
                CommandDelivery::reported(source_alive, || {
                    Some(CommandReceipt::Text(t!("admin-pause-done")))
                })
            },
            |_| {},
        ) {
            self.confirmation.set(None);
        }
    }
}

#[component]
pub(super) fn RuntimeSwitchCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    // 只把命令了结后的读取作为新意图的版本围栏；手动刷新期间保留原草稿和确认。
    let command_refresh = use_signal(|| 0_u64);
    let pause = PauseControlState {
        pending: use_pending_commands(command_refresh, |entity| entity == PAUSE_ENTITY),
        confirmation: use_signal(|| None),
        hours: use_signal(|| 2_i64),
        reason: use_signal(String::new),
        accepted: use_signal(|| false),
    };
    let snapshot = use_resource(move || {
        let generation = (refresh(), command_refresh());
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
            let generation = (refresh(), command_refresh());
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
                if (*refresh.peek(), *command_refresh.peek()) == generation {
                    refresh.set(generation.0.saturating_add(1));
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
            PendingCommandPanel { owner: pause.pending }
            div { role: "status",
                if (pause.accepted)() {
                    p { class: "admin-shell__banner", {t!("admin-pause-done")} }
                }
            }
            match current {
                None => rsx! {
                    AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-switches-loading"), detail: "" }
                },
                Some((_, Err(code))) => rsx! {
                    AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" }
                },
                Some(((_, loaded_generation), Ok(data))) => {
                    let (revision, paused, paused_until) = pause_state(&data);
                    rsx! {
                        ExternalAccessPauseControl { state: pause, revision, paused, paused_until, command_refresh, loaded_generation }
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
    command_refresh: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let PauseControlState { pending, mut confirmation, mut hours, mut reason, mut accepted } =
        state;
    let occupied = pending.contains(PAUSE_ENTITY);
    let waiting_for_snapshot = move || loaded_generation != command_refresh();
    let draft = confirmation();
    let stale = draft
        .as_ref()
        .is_some_and(|draft| draft.expected_revision != revision || draft.paused == paused);
    let entry_label = if paused { t!("admin-pause-resume") } else { t!("admin-pause-start") };
    let mut reason_edited = use_signal(|| false);
    let mut reason_invalid = use_signal(|| false);
    let announcer = use_field_announcer();

    let begin = move |event: FormEvent| {
        event.prevent_default();
        if pending.contains(PAUSE_ENTITY) || confirmation.peek().is_some() || waiting_for_snapshot()
        {
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
        confirmation.set(Some(PauseConfirmation {
            expected_revision: revision,
            paused: !paused,
            expires_at,
            reason: reason_text,
        }));
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
            if let Some(draft) = draft {
                p {
                    if draft.paused {
                        {t!("admin-pause-confirm-pause")} ": " {draft.expires_at.map_or_default(auction_time::Timestamp::to_rfc3339)}
                    } else {
                        {t!("admin-pause-confirm-resume")}
                    }
                }
                button {
                    class: if draft.paused { "admin-btn-danger" } else { "admin-btn-primary" },
                    r#type: "button",
                    disabled: occupied || waiting_for_snapshot() || stale,
                    onclick: move |_| {
                        if !waiting_for_snapshot() && !stale {
                            state.submit(draft.clone());
                        }
                    },
                    {t!("admin-pause-confirm")}
                }
                button {
                    class: "admin-btn-secondary",
                    r#type: "button",
                    onclick: move |_| confirmation.set(None),
                    {t!("admin-pause-cancel")}
                }
            } else {
                if !paused {
                    label {
                        {t!("admin-pause-duration")}
                        select {
                            name: "duration",
                            disabled: occupied,
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
                            disabled: occupied,
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
                    disabled: occupied || waiting_for_snapshot(),
                    "{entry_label}"
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

    use auction_contracts::{
        AdminExternalAccessPauseRequest, RuntimeSwitchIntentDto, RuntimeSwitchListResponse,
    };
    use auction_time::{TimeDelta, Timestamp};
    use dioxus::core::ElementId;
    use dioxus::prelude::*;
    use serde_json::json;

    use super::{
        ExternalAccessPauseControl, PAUSE_ENTITY, PAUSE_REASON_ID, PauseControlState,
        RuntimeSwitchCard, pause_refresh_millis, pause_state, reason_error_text,
    };
    use crate::api::{AdminApiFailure, ExternalAccessPauseAttempt};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, assert_field, button, click_button, has_live_text, mount_writer, settle_writer,
        submit_button_disabled, writer_form,
    };
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, use_pending_commands,
    };
    use crate::surface::realtime::{TestPage, text_attributes};

    pub(super) struct PauseRequests {
        reads: Cell<usize>,
        initial_reads: usize,
        refresh_reply: RecordRead<RuntimeSwitchListResponse>,
        writes: RefCell<Vec<String>>,
        accept_replay: bool,
    }

    impl PauseRequests {
        fn new() -> Self {
            Self {
                reads: Cell::new(0),
                initial_reads: 1,
                refresh_reply: RecordRead::new(),
                writes: RefCell::new(Vec::new()),
                accept_replay: false,
            }
        }

        pub(super) async fn read(&self) -> Result<RuntimeSwitchListResponse, String> {
            let prior = self.reads.replace(self.reads.get() + 1);
            if prior < self.initial_reads {
                return Ok(snapshot("clear", None, Timestamp::now()));
            }
            self.refresh_reply.fetch().await
        }

        fn finish_refresh(&self) {
            let mut data = snapshot("set", None, Timestamp::now());
            data.database_intent[0].revision = 8;
            self.refresh_reply.respond(Ok(data));
        }

        fn write(
            &self,
            attempt: &AdminExternalAccessPauseRequest,
        ) -> Result<RuntimeSwitchIntentDto, AdminApiFailure> {
            let mut writes = self.writes.borrow_mut();
            writes.push(serde_json::to_string(attempt).unwrap());
            let failure = match writes.len() {
                1 => AdminApiFailure::from_http_body(
                    503,
                    r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                ),
                2 | 4 => AdminApiFailure::from_http_body(
                    429,
                    r#"{"code":"rate_limited","message":"请求过多","data":null,"request_id":null}"#,
                ),
                3 if self.accept_replay => {
                    let mut accepted = snapshot("set", attempt.expires_at, Timestamp::now());
                    accepted.database_intent[0].revision = 8;
                    return Ok(accepted.database_intent.remove(0));
                }
                3 => AdminApiFailure::from_http_body(
                    409,
                    r#"{"code":"runtime_switch_conflict","message":"版本已变化","data":null,"request_id":null}"#,
                ),
                _ => panic!("收到额外的暂停命令"),
            };
            Err(failure)
        }

        fn install(self: &Rc<Self>) {
            use_context_provider(|| self.clone());
            let requests = self.clone();
            use_hook(move || {
                ExternalAccessPauseAttempt::stub_submit(move |attempt| requests.write(attempt));
            });
        }
    }

    fn mount_card(requests: Rc<PauseRequests>) -> (TestPage, ElementId, ElementId) {
        let (page, edits) = mount_writer(move || {
            requests.install();
            rsx! { RuntimeSwitchCard {} }
        });
        let reason = text_attributes(&edits, "id")
            .find_map(|(id, value)| (value == PAUSE_REASON_ID).then_some(id))
            .unwrap();
        (page, writer_form(&edits), reason)
    }

    #[test]
    fn an_open_pause_confirmation_rejects_clicks_until_its_read_is_current() {
        let requests = Rc::new(PauseRequests::new());
        let (mut page, initial) = mount_writer({
            let requests = requests.clone();
            move || {
                requests.install();
                let mut command_refresh = use_signal(|| 0);
                let mut loaded_generation = use_signal(|| 0);
                let state = PauseControlState {
                    pending: use_pending_commands(command_refresh, |entity| entity == PAUSE_ENTITY),
                    confirmation: use_signal(|| None),
                    hours: use_signal(|| 2),
                    reason: use_signal(|| "演练".to_owned()),
                    accepted: use_signal(|| false),
                };
                rsx! {
                    button { onclick: move |_| command_refresh.set(1), {"开始命令后重读".to_owned()} }
                    button { onclick: move |_| loaded_generation.set(1), {"完成命令后重读".to_owned()} }
                    ExternalAccessPauseControl {
                        state, revision: 7, paused: false, paused_until: None,
                        command_refresh, loaded_generation: loaded_generation(),
                    }
                }
            }
        });
        page.dispatch("submit", writer_form(&initial), "");
        assert!(!button(&page, "确认").unwrap().1);
        click_button(&mut page, "开始命令后重读");
        let (confirm, disabled) = button(&page, "确认").unwrap();
        page.dispatch("click", confirm, "");
        assert!(requests.writes.borrow().is_empty());
        assert!(disabled);
        click_button(&mut page, "完成命令后重读");
        click_button(&mut page, "确认");
        assert_eq!(requests.writes.borrow().len(), 1);
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
        assert!(has_live_text(&page, "曾有请求结果未知，请保留原命令，勿为同一实体新建操作。"));
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
        let first: AdminExternalAccessPauseRequest = serde_json::from_str(&writes[0]).unwrap();
        let latest: AdminExternalAccessPauseRequest = serde_json::from_str(&writes[3]).unwrap();
        assert_eq!(latest.expected_revision, 8);
        assert_ne!(latest.command_id, first.command_id);
    }

    #[test]
    fn pause_cannot_replace_unknown_after_leaving_and_returning() {
        let requests = Rc::new(PauseRequests::new());
        let (mut page, initial) = mount_writer({
            let requests = requests.clone();
            move || {
                requests.install();
                let host = use_context::<PendingCommandHost>();
                let mut visible = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| visible.toggle(), {"切换测试路由".to_owned()} }
                    PendingCommandNotices { host }
                    if visible() { RuntimeSwitchCard {} }
                }
            }
        });
        let reason = |edits: &[dioxus::core::Mutation]| {
            text_attributes(edits, "id")
                .find_map(|(id, value)| (value == PAUSE_REASON_ID).then_some(id))
                .unwrap()
        };
        page.dispatch("input", reason(&initial), "演练");
        page.dispatch("submit", writer_form(&initial), "");
        click_button(&mut page, "确认");
        click_button(&mut page, "切换测试路由");
        click_button(&mut page, "切换测试路由");
        requests.finish_refresh();
        let remounted = settle_writer(&mut page);
        page.dispatch("input", reason(&remounted), "重新输入的原因");
        page.dispatch("submit", writer_form(&remounted), "");
        assert!(button(&page, "确认").is_none());
        assert!(button(&page, "原样重试").is_some());
        assert_eq!(requests.writes.borrow().len(), 1);
        click_button(&mut page, "原样重试");
        let writes = requests.writes.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], writes[1]);
    }

    #[test]
    fn remounted_pause_waits_for_the_read_after_the_original_command_settles() {
        for accept_replay in [true, false] {
            let requests =
                Rc::new(PauseRequests { initial_reads: 2, accept_replay, ..PauseRequests::new() });
            let (mut page, initial) = mount_writer({
                let requests = requests.clone();
                move || {
                    requests.install();
                    let host = use_context::<PendingCommandHost>();
                    let mut on_config = use_signal(|| true);
                    rsx! {
                        button { onclick: move |_| on_config.toggle(), {"切换测试路由".to_owned()} }
                        PendingCommandNotices { host }
                        if on_config() { RuntimeSwitchCard {} }
                    }
                }
            });
            let reason = |edits: &[dioxus::core::Mutation]| {
                text_attributes(edits, "id")
                    .find_map(|(id, value)| (value == PAUSE_REASON_ID).then_some(id))
                    .unwrap()
            };
            page.dispatch("input", reason(&initial), "第一次暂停");
            page.dispatch("submit", writer_form(&initial), "");
            click_button(&mut page, "确认");
            click_button(&mut page, "切换测试路由");
            let remounted = click_button(&mut page, "切换测试路由");
            assert_eq!(requests.reads.get(), 2);
            assert!(has_live_text(&page, "暂停外部访问…"));
            let form = writer_form(&remounted);
            click_button(&mut page, "原样重试");
            click_button(&mut page, "原样重试");
            assert!(button(&page, "原样重试").is_none());
            assert_eq!(requests.reads.get(), 3);
            assert_eq!(submit_button_disabled(&page, "暂停外部访问…"), Some(true));
            page.dispatch("input", reason(&remounted), "恢复访问");
            page.dispatch("submit", form, "");
            assert!(button(&page, "确认").is_none());
            assert_eq!(requests.writes.borrow().len(), 3);

            requests.finish_refresh();
            settle_writer(&mut page);
            assert_eq!(submit_button_disabled(&page, "恢复外部访问…"), Some(false));
            page.dispatch("submit", form, "");
            click_button(&mut page, "确认");
            let writes = requests.writes.borrow();
            assert_eq!(writes.len(), 4);
            assert_eq!(writes[0], writes[2]);
            let original: AdminExternalAccessPauseRequest =
                serde_json::from_str(&writes[0]).unwrap();
            let latest: AdminExternalAccessPauseRequest = serde_json::from_str(&writes[3]).unwrap();
            assert_ne!(latest.command_id, original.command_id);
            assert_eq!(latest.expected_revision, 8);
            assert!(!latest.paused);
        }
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
    use dioxus::core::{AttributeValue, ElementId, Mutation, ScopeId};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        ExternalAccessPauseControl, PAUSE_ENTITY, PAUSE_REASON_ID, PauseControlState,
        reason_error_text,
    };
    use crate::api::ExternalAccessPauseAttempt;
    use crate::surface::ADMIN_LOCALE_CATALOG;
    use crate::surface::config::test_form::{FormBrowser, dispatch, listener};
    use crate::surface::pending::{use_pending_command_host, use_pending_commands};

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
        use_pending_command_host();
        let command_refresh = use_signal(|| 0);
        let state = PauseControlState {
            pending: use_pending_commands(command_refresh, |entity| entity == PAUSE_ENTITY),
            confirmation: use_signal(|| None),
            hours: use_signal(|| 2),
            reason: use_signal(|| {
                "界".repeat(ExternalAccessPauseAttempt::MAX_REASON_BYTES / 3 + 1)
            }),
            accepted: use_signal(|| false),
        };
        rsx! { ExternalAccessPauseControl { state, revision: 7, paused: false, paused_until: None, command_refresh, loaded_generation: 0 } }
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
