//! 邮件模板卡片：出站通知正文保存在数据库，与 runtime 文件配置无关。逐个模板编辑，两阶段
//! 确认后保存或恢复内置默认正文；结果未知时只允许原样重试。

use std::collections::{BTreeMap, BTreeSet};

use auction_contracts::AdminMailTemplateDto;
use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use crate::api::{
    MailTemplateAttempt, MailTemplateBodyProblem, fetch_mail_templates, mail_template_body_problem,
    submit_mail_template,
};
use crate::surface::format_admin_time;
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandPanel, PendingCommands, try_set_if_alive,
    use_pending_commands,
};

const TEMPLATE_ENTITY_PREFIX: &str = "mail-template:";

fn template_entity(key: &str) -> String {
    format!("{TEMPLATE_ENTITY_PREFIX}{key}")
}

#[derive(Clone)]
struct TemplateDraft {
    revision: i64,
    body: String,
}

impl TemplateDraft {
    fn save_revision(&self, published: i64, rejected: bool) -> Option<i64> {
        (!rejected && self.revision == published).then_some(self.revision)
    }
}

#[derive(Clone, PartialEq)]
struct TemplateConfirmation {
    key: String,
    expected_revision: i64,
    body: Option<String>,
}

type TemplateListResult = Result<Vec<AdminMailTemplateDto>, String>;
type TemplateListRead = (u64, TemplateListResult);

/// 卡片持有尚未提交的草稿和确认，已提交的模板意图归认证会话。
#[derive(Clone, Copy, PartialEq)]
struct TemplateEditState {
    pending: PendingCommands,
    confirmation: Signal<Option<TemplateConfirmation>>,
    /// 正文与首次编辑时的版本一起保留，列表刷新不得替换草稿的版本围栏。
    drafts: Signal<BTreeMap<String, TemplateDraft>>,
    /// 服务端冲突按模板保留，避免另一模板的操作清除这一次拒绝。
    conflicts: Signal<BTreeSet<String>>,
    /// 最近一次被数据库接受的模板键，用于显示成功提示。
    accepted: Signal<Option<String>>,
    snapshot: Resource<TemplateListRead>,
    refresh: Signal<u64>,
}

impl TemplateEditState {
    fn list_ready(self) -> bool {
        // 命令了结立即使旧版本失效，Resource 尚未开始新一轮读取时也不能提交。
        let generation = (self.refresh)();
        *self.snapshot.state().read() == UseResourceState::Ready
            && self.snapshot.read().as_ref().is_some_and(|(loaded, _)| *loaded == generation)
    }

    fn current_revision(self, key: &str) -> Option<i64> {
        let snapshot = self.snapshot.peek();
        let templates = snapshot.as_ref()?.1.as_ref().ok()?;
        templates.iter().find(|template| template.key == key).map(|template| template.revision)
    }

    fn can_edit(self, key: &str, revision: i64) -> bool {
        !self.pending.contains(&template_entity(key))
            && !self.pending.contains_local(TEMPLATE_ENTITY_PREFIX)
            && self.confirmation.peek().is_none()
            && !self.conflicts.peek().contains(key)
            && self.current_revision(key) == Some(revision)
    }

    fn begin_save(mut self, template: &AdminMailTemplateDto) -> Option<MailTemplateBodyProblem> {
        let key = &template.key;
        if !self.can_edit(key, template.revision) || !self.list_ready() {
            return None;
        }
        let draft = self.drafts.peek().get(key).cloned()?;
        let revision = draft.save_revision(template.revision, false)?;
        if let Some(problem) = mail_template_body_problem(template, &draft.body) {
            return Some(problem);
        }
        self.accepted.set(None);
        self.confirmation.set(Some(TemplateConfirmation {
            key: key.clone(),
            expected_revision: revision,
            body: Some(draft.body),
        }));
        None
    }

    fn submit(mut self, confirmation: TemplateConfirmation) {
        if self.confirmation.peek().as_ref() != Some(&confirmation)
            || self.pending.contains_local(TEMPLATE_ENTITY_PREFIX)
            || self.conflicts.peek().contains(&confirmation.key)
            || !self.list_ready()
            || self.current_revision(&confirmation.key) != Some(confirmation.expected_revision)
        {
            return;
        }
        let key = confirmation.key.clone();
        let entity = template_entity(&key);
        let action = if confirmation.body.is_some() {
            t!("admin-mail-template-save")
        } else {
            t!("admin-mail-template-reset")
        };
        let action = format!("{action} · {}", template_label(&key));
        // 同键保存与恢复默认共用实体围栏；重试只用确认的版本与正文。
        if self.pending.submit_with_receipt(
            entity,
            action,
            move |command_id| {
                let confirmation = confirmation.clone();
                async move {
                    submit_mail_template(&MailTemplateAttempt {
                        command_id,
                        key: confirmation.key,
                        expected_revision: confirmation.expected_revision,
                        body: confirmation.body,
                    })
                    .await
                }
            },
            move |template: AdminMailTemplateDto, source_alive| {
                let displayed =
                    source_alive && try_set_if_alive(self.accepted, Some(template.key.clone()));
                if displayed {
                    let mut drafts = self.drafts;
                    let mut snapshot = self.snapshot;
                    drafts.write().remove(&template.key);
                    // 接受后的旧投影不能成为下一份草稿的基准。
                    snapshot.clear();
                }
                CommandDelivery::reported(displayed, || {
                    Some(CommandReceipt::Text(t!("admin-mail-template-done")))
                })
            },
            move |code| {
                if code == "mail_template_conflict" {
                    // 账本核对和修订围栏使旧命令了结，未保存草稿仍保留原版本。
                    let mut conflicts = self.conflicts;
                    conflicts.write().insert(key.clone());
                }
            },
        ) {
            self.confirmation.set(None);
        }
    }

    fn discard(mut self, key: &str, conflicted: bool) {
        if self.pending.contains(&template_entity(key))
            || self.pending.contains_local(TEMPLATE_ENTITY_PREFIX)
        {
            return;
        }
        if self.confirmation.peek().as_ref().is_some_and(|pending| pending.key == key) {
            self.confirmation.set(None);
        }
        self.drafts.write().remove(key);
        self.conflicts.write().remove(key);
        if conflicted {
            self.snapshot.clear();
            self.snapshot.restart();
        }
    }
}

fn use_template_edit_state(
    snapshot: Resource<TemplateListRead>,
    refresh: Signal<u64>,
) -> TemplateEditState {
    TemplateEditState {
        pending: use_pending_commands(refresh, |entity| entity.starts_with(TEMPLATE_ENTITY_PREFIX)),
        confirmation: use_signal(|| None),
        drafts: use_signal(BTreeMap::new),
        conflicts: use_signal(BTreeSet::new),
        accepted: use_signal(|| None),
        snapshot,
        refresh,
    }
}

#[component]
pub(super) fn MailTemplateCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let snapshot = use_resource(move || {
        let generation = refresh();
        async move {
            #[cfg(test)]
            if let Some(reads) = try_consume_context::<tests::TemplateReads>() {
                return (generation, reads.next());
            }
            (generation, fetch_mail_templates().await)
        }
    });
    let state = use_template_edit_state(snapshot, refresh);
    let current = snapshot.read_unchecked().clone();
    rsx! {
        section { class: "admin-mail-templates admin-panel",
            h2 { {t!("admin-runtime-section-templates")} }
            p { class: "admin-panel__note", {t!("admin-mail-templates-note")} }
            button {
                class: "admin-btn-secondary",
                r#type: "button",
                onclick: move |_| refresh.set(refresh().saturating_add(1)),
                {t!("admin-mail-templates-refresh")}
            }
            PendingCommandPanel { owner: state.pending }
            div { role: "status",
                if let Some(key) = (state.accepted)() {
                    p { class: "admin-shell__banner",
                        {template_label(&key)} " · " {t!("admin-mail-template-done")}
                    }
                }
            }
            match current {
                None => rsx! {
                    AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-mail-templates-loading"), detail: "" }
                },
                Some((_, Err(code))) => rsx! {
                    AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" }
                },
                Some((_, Ok(templates))) => rsx! {
                    div { class: "admin-mail-templates__list",
                        for template in templates {
                            MailTemplateEditor { key: "{template.key}", template, state }
                        }
                    }
                },
            }
        }
    }
}

#[component]
fn MailTemplateEditor(template: AdminMailTemplateDto, state: TemplateEditState) -> Element {
    let TemplateEditState { mut confirmation, mut drafts, mut accepted, .. } = state;
    let key = template.key.clone();
    let label = template_label(&key);
    let occupied = state.pending.contains(&template_entity(&key));
    let card_busy = state.pending.contains_local(TEMPLATE_ENTITY_PREFIX);
    let pending_confirmation = confirmation();
    let own_confirmation = pending_confirmation.clone().filter(|pending| pending.key == key);
    let confirming = own_confirmation.is_some();
    let busy = occupied || card_busy || pending_confirmation.is_some();
    let list_ready = state.list_ready();
    let draft = drafts.read().get(&key).cloned();
    let dirty = draft.is_some();
    let rejected = state.conflicts.read().contains(&key);
    let conflicted = draft
        .as_ref()
        .map_or(rejected, |draft| draft.save_revision(template.revision, rejected).is_none())
        || own_confirmation
            .as_ref()
            .is_some_and(|pending| pending.expected_revision != template.revision);
    let body = draft.map_or_else(|| template.body.clone(), |draft| draft.body);
    let mut show_problem = use_signal(|| false);
    let visible_problem = if show_problem() && dirty && !conflicted {
        mail_template_body_problem(&template, &body)
    } else {
        None
    };
    let just_accepted = accepted().as_deref() == Some(key.as_str());
    let expanded = dirty || confirming || conflicted || just_accepted || occupied;

    let saved_body = template.body.clone();
    let input_key = key.clone();
    let revision = template.revision;
    let edit = move |event: FormEvent| {
        if !state.can_edit(&input_key, revision) {
            return;
        }
        let value = event.value();
        let mut drafts = drafts.write();
        if drafts.get(&input_key).is_some_and(|draft| draft.revision != revision) {
            return;
        }
        accepted.set(None);
        // 只在同一基准版本下把改回原文视为撤销编辑。
        if value == saved_body {
            drafts.remove(&input_key);
            show_problem.set(false);
        } else {
            drafts
                .entry(input_key.clone())
                .or_insert_with(|| TemplateDraft { revision, body: String::new() })
                .body = value;
        }
    };

    let field_id = format!("admin-mail-template-{key}");
    let save_field_id = field_id.clone();
    let save_template = template.clone();
    let begin_save = move |event: FormEvent| {
        event.prevent_default();
        let invalid = state.begin_save(&save_template).is_some();
        show_problem.set(invalid);
        if invalid {
            focus_template_body_after_render(save_field_id.clone());
        }
    };
    let reset_key = key.clone();
    let begin_reset = move |_| {
        if !state.can_edit(&reset_key, revision)
            || !state.list_ready()
            || drafts.peek().get(&reset_key).is_some_and(|draft| draft.revision != revision)
        {
            return;
        }
        accepted.set(None);
        confirmation.set(Some(TemplateConfirmation {
            key: reset_key.clone(),
            expected_revision: revision,
            body: None,
        }));
    };
    let discard_key = key.clone();
    let discard = move |_| {
        show_problem.set(false);
        state.discard(&discard_key, conflicted);
    };

    let status = if !template.customized {
        t!("admin-mail-template-status-default")
    } else if template.usable {
        t!("admin-mail-template-status-custom")
    } else {
        t!("admin-mail-template-status-unusable")
    };
    let pm_identity = key == "lot_watch_pm";
    let rejection_reason = key == "auction_equipment_rejected";
    let watch_body = matches!(key.as_str(), "lot_watch" | "lot_watch_pm");
    let mut required_placeholders = template.required_placeholders.clone();
    if pm_identity && !required_placeholders.iter().any(|name| name == "notification_id") {
        required_placeholders.push("notification_id".to_owned());
    }
    let optional_placeholders: Vec<_> = template
        .allowed_placeholders
        .iter()
        .filter(|name| {
            !required_placeholders.contains(name)
                && !(rejection_reason && matches!(name.as_str(), "reason_code" | "reason_text"))
        })
        .cloned()
        .collect();
    let help_id = format!("{field_id}-help");
    let error_id = format!("{field_id}-error");
    let description =
        if visible_problem.is_some() { format!("{help_id} {error_id}") } else { help_id.clone() };

    rsx! {
        details { class: "admin-runtime-section admin-mail-template", open: expanded,
            summary {
                span { id: "{field_id}-label", "{label}" }
                span { class: "admin-mail-template__status", "{status}" }
            }
            form { class: "admin-runtime-section__body",
                action: "/api/v1/admin/mail-templates/{key}",
                method: "post",
                aria_labelledby: "{field_id}-label",
                novalidate: true,
                onsubmit: begin_save,
                if let (Some(actor), Some(time)) = (template.updated_by.clone(), template.updated_at) {
                    p { class: "admin-runtime-section-note",
                        {t!("admin-mail-template-revision", revision: template.revision, actor: actor, time: format_admin_time(&time.to_rfc3339()))}
                    }
                }
                div { id: help_id,
                    p { class: "admin-runtime-section-note",
                        if template.allowed_placeholders.is_empty() {
                            {t!("admin-mail-template-no-placeholders")}
                        } else {
                            {t!("admin-mail-template-required")} ": " {placeholder_list(&required_placeholders)}
                            " · "
                            {t!("admin-mail-template-optional")} ": " {placeholder_list(&optional_placeholders)}
                        }
                    }
                    if pm_identity {
                        p { class: "admin-runtime-section-note", {problem_message(MailTemplateBodyProblem::MissingNotificationIdentity)} }
                    }
                    if rejection_reason {
                        p { class: "admin-runtime-section-note", {problem_message(MailTemplateBodyProblem::MissingRejectionReason)} }
                    }
                    if watch_body {
                        p { class: "admin-runtime-section-note", {t!("admin-mail-template-watch-limit")} }
                    }
                }
                div { class: "admin-runtime-field admin-runtime-field--template",
                    label { r#for: field_id.clone(), "{label}" span { aria_hidden: "true", " *" } }
                    textarea {
                        id: field_id,
                        name: "body",
                        autocomplete: "off",
                        spellcheck: "false",
                        rows: "8",
                        disabled: busy && !conflicted,
                        readonly: conflicted,
                        required: true,
                        aria_invalid: visible_problem.is_some().to_string(),
                        aria_describedby: description,
                        value: body,
                        oninput: edit,
                    }
                }
                if let Some(problem) = visible_problem {
                    p { class: "admin-runtime-section-warning", id: error_id, {problem_message(problem)} }
                }
                if conflicted {
                    p { class: "admin-runtime-section-warning", role: "status", {t!("admin-mail-template-draft-conflict")} }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: occupied || card_busy || (busy && !confirming),
                        onclick: discard,
                        {t!("admin-mail-template-discard-reload")}
                    }
                } else if let Some(pending) = own_confirmation {
                    p {
                        if pending.body.is_some() {
                            {t!("admin-mail-template-confirm-save")}
                        } else {
                            {t!("admin-mail-template-confirm-reset")}
                        }
                    }
                    div { class: "admin-mail-template__actions",
                        button {
                            class: "admin-btn-primary",
                            r#type: "button",
                            disabled: occupied || card_busy || !list_ready,
                            onclick: move |_| state.submit(pending.clone()),
                            {t!("admin-mail-template-confirm")}
                        }
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            onclick: move |_| confirmation.set(None),
                            {t!("admin-mail-template-cancel")}
                        }
                    }
                } else {
                    div { class: "admin-mail-template__actions",
                        button {
                            class: "admin-btn-primary",
                            r#type: "submit",
                            disabled: busy || !list_ready || !dirty,
                            {t!("admin-mail-template-save")}
                        }
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy || !dirty,
                            onclick: discard,
                            {t!("admin-mail-template-discard")}
                        }
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy || !list_ready || !template.customized,
                            onclick: begin_reset,
                            {t!("admin-mail-template-reset")}
                        }
                    }
                }
            }
        }
    }
}

fn focus_template_body_after_render(_field_id: String) {
    #[cfg(target_arch = "wasm32")]
    spawn(async move {
        auction_web_shared::timer::TimeoutFuture::new(0).await;
        auction_web_shared::a11y::focus_element_by_id(&_field_id);
    });
}

/// 占位符按正文里的写法展示（带花括号）；没有时显示占位符号。
fn placeholder_list(names: &[String]) -> String {
    if names.is_empty() {
        return "—".to_owned();
    }
    names.iter().map(|name| format!("{{{name}}}")).collect::<Vec<_>>().join(" ")
}

fn problem_message(problem: MailTemplateBodyProblem) -> String {
    match problem {
        MailTemplateBodyProblem::Format => t!("admin-mail-template-problem-format"),
        MailTemplateBodyProblem::Unclosed => t!("admin-mail-template-problem-unclosed"),
        MailTemplateBodyProblem::UnknownPlaceholder => t!("admin-mail-template-problem-unknown"),
        MailTemplateBodyProblem::MissingPlaceholder => t!("admin-mail-template-problem-missing"),
        MailTemplateBodyProblem::MissingRejectionReason => {
            t!("admin-mail-template-reason-required", code: "{reason_code}", text: "{reason_text}")
        }
        MailTemplateBodyProblem::MissingNotificationIdentity => {
            t!("admin-mail-template-notification-identity", sentence: MailTemplateBodyProblem::NOTIFICATION_IDENTITY)
        }
        MailTemplateBodyProblem::WatchBodyTooLong => t!("admin-mail-template-watch-limit"),
        MailTemplateBodyProblem::RequestTooLarge => t!("admin-error-body-too-large"),
    }
}

/// 模板键到界面名称；服务端新增而本页尚不认识的键直接显示键名。
fn template_label(key: &str) -> String {
    match key {
        "auction_assets_rejected" => t!("admin-runtime-template-auction-assets"),
        "auction_no_attachments" => t!("admin-runtime-template-no-attachments"),
        "auction_equipment_rejected" => t!("admin-runtime-template-equipment-rejected"),
        "user_assets_rejected" => t!("admin-runtime-template-user-assets"),
        "register_key" => t!("admin-runtime-template-register-key"),
        "register_result" => t!("admin-runtime-template-register-result"),
        "rebind_result" => t!("admin-runtime-template-rebind-result"),
        "rekey_key" => t!("admin-runtime-template-rekey-key"),
        "rekey_result" => t!("admin-runtime-template-rekey-result"),
        "settlement_winner_equipment_cod" => t!("admin-runtime-template-winner-cod"),
        "settlement_payment_carrier" => t!("admin-runtime-template-payment-carrier"),
        "settlement_payment_reminder" => t!("admin-runtime-template-payment-reminder"),
        "settlement_equipment_handoff" => t!("admin-runtime-template-equipment-handoff"),
        "settlement_seller_payout" => t!("admin-runtime-template-seller-payout"),
        "settlement_buyer_refund" => t!("admin-runtime-template-buyer-refund"),
        "lot_watch" => t!("admin-runtime-template-lot-watch"),
        "lot_watch_pm" => t!("admin-runtime-template-lot-watch-pm"),
        "operations_alert" => t!("admin-runtime-template-operations-alert"),
        _ => key.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, cell::RefCell, collections::VecDeque, rc::Rc};

    use auction_contracts::{AdminMailTemplateDto, AdminMailTemplateUpdateRequest};
    use dioxus::prelude::*;
    use uuid::Uuid;

    use super::{
        MailTemplateCard, MailTemplateEditor, TemplateDraft, TemplateEditState, TemplateListResult,
        placeholder_list, template_entity, use_template_edit_state,
    };
    use crate::api::{
        AdminApiFailure, AdminSessionWritePermission, AdminWriteEndpoint, AdminWriteFailure,
        MailTemplateAttempt, MailTemplateBodyProblem,
    };
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, init_i18n, mount_writer, writer_form,
    };
    use crate::surface::pending::{
        PendingCommandHost, PendingCommandNotices, use_pending_command_host,
    };
    use crate::surface::realtime::text_attributes;

    #[derive(Default)]
    struct EditorHarness {
        editor: Cell<Option<TemplateEditState>>,
        latest: Cell<Option<Signal<AdminMailTemplateDto>>>,
        requests: RefCell<Vec<String>>,
        failures: RefCell<VecDeque<AdminWriteFailure>>,
    }

    fn published(revision: i64) -> AdminMailTemplateDto {
        AdminMailTemplateDto {
            key: "auction_no_attachments".to_owned(),
            customized: true,
            usable: true,
            body: format!("服务器正文 {revision}"),
            default_body: "默认正文".to_owned(),
            revision,
            updated_by: Some("admin".to_owned()),
            updated_at: None,
            allowed_placeholders: Vec::new(),
            required_placeholders: Vec::new(),
        }
    }

    #[derive(Clone)]
    pub(super) struct TemplateReads(Rc<RefCell<VecDeque<TemplateListResult>>>);

    impl TemplateReads {
        pub(super) fn next(&self) -> TemplateListResult {
            self.0.borrow_mut().pop_front().expect("模板读取对应脚本回执")
        }
    }

    #[test]
    fn template_receipt_remains_visible_when_the_read_after_save_fails() {
        let failure = AdminApiFailure::from_http_body(
            503,
            r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
        );
        let reads = TemplateReads(Rc::new(RefCell::new(VecDeque::from([
            Ok(vec![published(5)]),
            Err(failure.code),
            Ok(vec![published(6)]),
        ]))));
        let (mut page, initial) = mount_writer(move || {
            use_context_provider(|| reads.clone());
            use_hook(|| {
                MailTemplateAttempt::stub_submit(|body| {
                    let request: AdminMailTemplateUpdateRequest =
                        serde_json::from_str(body).unwrap();
                    assert_eq!(request.expected_revision, 5);
                    Ok(published(6))
                });
            });
            rsx! { MailTemplateCard {} }
        });
        let field = text_attributes(&initial, "value")
            .find_map(|(id, value)| (value == "服务器正文 5").then_some(id))
            .unwrap();
        assert!(!has_live_text(&page, "数据库已接受；各角色约 5 秒内生效。"));
        page.dispatch("input", field, "已更新正文");
        page.dispatch("submit", writer_form(&initial), "");
        click_button(&mut page, "确认");
        assert!(has_live_text(&page, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&page, "数据库已接受；各角色约 5 秒内生效。"));
        click_button(&mut page, "刷新");
        assert!(!has_live_text(&page, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&page, "数据库已接受；各角色约 5 秒内生效。"));
    }

    fn test_editor() -> Element {
        init_i18n();
        let permission = use_memo(|| true);
        use_context_provider(|| AdminSessionWritePermission(permission));
        use_pending_command_host();
        let harness = consume_context::<Rc<EditorHarness>>();
        use_hook({
            let harness = harness.clone();
            move || {
                MailTemplateAttempt::stub_submit(move |request| {
                    harness.requests.borrow_mut().push(request.to_owned());
                    match harness.failures.borrow_mut().pop_front() {
                        Some(failure) => Err(failure),
                        None => Ok(published(7)),
                    }
                });
            }
        });
        let latest = use_signal(|| published(5));
        let refresh = use_signal(|| 0_u64);
        let snapshot = use_resource(move || {
            let generation = refresh();
            let template = latest();
            async move { (generation, Ok(vec![template])) }
        });
        harness.latest.set(Some(latest));
        harness.editor.set(Some(use_template_edit_state(snapshot, refresh)));
        rsx! { div {} }
    }

    #[component]
    fn TemplateRoute() -> Element {
        let refresh = use_signal(|| 0_u64);
        let snapshot = use_resource(move || {
            let generation = refresh();
            async move { (generation, Ok(vec![published(5)])) }
        });
        let state = use_template_edit_state(snapshot, refresh);
        rsx! {
            MailTemplateEditor { state, template: published(5) }
        }
    }

    #[test]
    fn template_cannot_replace_unknown_after_leaving_and_returning() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let (mut page, initial) = mount_writer({
            let requests = requests.clone();
            move || {
                let requests = requests.clone();
                use_hook(move || {
                    MailTemplateAttempt::stub_submit(move |body| {
                        requests.borrow_mut().push(body.to_owned());
                        Err(AdminWriteFailure::from_test_http(
                            AdminWriteEndpoint::MailTemplate,
                            503,
                            r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#,
                        ))
                    });
                });
                let host = use_context::<PendingCommandHost>();
                let mut visible = use_signal(|| true);
                rsx! {
                    button { onclick: move |_| visible.toggle(), {"切换测试路由".to_owned()} }
                    PendingCommandNotices { host }
                    if visible() { TemplateRoute {} }
                }
            }
        });
        let body_field = |edits: &[dioxus::core::Mutation]| {
            text_attributes(edits, "value")
                .find_map(|(id, value)| (value == "服务器正文 5").then_some(id))
                .unwrap()
        };
        page.dispatch("input", body_field(&initial), "第一次正文");
        page.dispatch("submit", writer_form(&initial), "");
        click_button(&mut page, "确认");
        assert_eq!(requests.borrow().len(), 1);
        click_button(&mut page, "切换测试路由");
        let remounted = click_button(&mut page, "切换测试路由");
        page.dispatch("input", body_field(&remounted), "第二次正文");
        page.dispatch("submit", writer_form(&remounted), "");
        assert!(button(&page, "确认").is_none());
        assert_eq!(requests.borrow().len(), 1);
        click_button(&mut page, "原样重试");
        let writes = requests.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0], writes[1]);
    }

    #[test]
    fn placeholders_are_shown_the_way_they_are_written_in_the_body() {
        assert_eq!(
            placeholder_list(&["amount".to_owned(), "reference".to_owned()]),
            "{amount} {reference}"
        );
        assert_eq!(placeholder_list(&[]), "—");
    }

    #[test]
    fn a_refreshed_list_cannot_rebase_a_live_draft() {
        let draft = TemplateDraft { revision: 5, body: "管理员的草稿".into() };
        assert_eq!(draft.save_revision(5, false), Some(5));
        assert_eq!(draft.save_revision(6, false), None);
        assert_eq!(draft.save_revision(0, false), None);
        assert_eq!(draft.body, "管理员的草稿");

        let reloaded = TemplateDraft { revision: 6, body: "最新版本".into() };
        assert_eq!(reloaded.save_revision(6, false), Some(6));
    }

    #[test]
    fn a_version_conflict_blocks_save_even_before_refresh() {
        let draft = TemplateDraft { revision: 5, body: "仍未保存的草稿".into() };
        assert_eq!(draft.save_revision(5, true), None);
        assert_eq!(draft.save_revision(5, false), Some(5));
    }

    #[test]
    fn only_a_complete_first_template_413_releases_the_entity() {
        for (body, pending) in [
            (
                r#"{"code":"body_too_large","message":"请求体过大","data":null,"request_id":null}"#,
                false,
            ),
            (r#"{"code":"body_too_large","message":"请求体过大","data":null}"#, true),
        ] {
            let harness = Rc::new(EditorHarness::default());
            harness.failures.borrow_mut().push_back(AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                413,
                body,
            ));
            let mut dom = VirtualDom::new(test_editor).with_root_context(harness.clone());
            dom.rebuild_in_place();
            dom.process_events();
            let mut editor = harness.editor.get().unwrap();
            let template = published(5);
            let key = &template.key;
            dom.in_scope(ScopeId::APP, || {
                editor.drafts.write().insert(
                    key.clone(),
                    TemplateDraft { revision: 5, body: "保留草稿".to_owned() },
                );
                assert_eq!(editor.begin_save(&template), None);
                let confirmation = editor.confirmation.peek().clone().unwrap();
                editor.submit(confirmation);
            });
            dom.process_events();
            let requests = harness.requests.borrow();
            assert_eq!(requests.len(), 1);
            let sent: AdminMailTemplateUpdateRequest = serde_json::from_str(&requests[0]).unwrap();
            assert_eq!(sent.expected_revision, 5);
            assert_eq!(sent.body.as_deref(), Some("保留草稿"));
            assert_eq!(editor.pending.contains(&template_entity(key)), pending);
            assert_eq!(editor.drafts.peek()[key].body, "保留草稿");
            assert!(editor.accepted.peek().is_none());
        }
    }

    #[test]
    fn template_retries_send_the_frozen_revision_until_a_ledger_conflict_allows_reload() {
        let harness = Rc::new(EditorHarness::default());
        let attempts = [
            AdminWriteFailure::outcome_unknown("request_timeout"),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                401,
                r#"{"code":"unauthenticated","message":"请求已拒绝","data":null,"request_id":null}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                429,
                r#"{"code":"rate_limited","message":"请求已拒绝","data":null,"request_id":null}"#,
            ),
            AdminWriteFailure::outcome_unknown("network_error"),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                413,
                r#"{"code":"body_too_large","message":"请求体过大","data":null,"request_id":null}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                413,
                r#"{"code":"body_too_large","message":"请求体过大","data":null}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":7}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":null,"extra":true}"#,
            ),
            AdminWriteFailure::from_test_http(
                AdminWriteEndpoint::MailTemplate,
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":null}"#,
            ),
        ];
        let attempt_count = attempts.len();
        harness.failures.borrow_mut().extend(attempts);
        let mut dom = VirtualDom::new(test_editor).with_root_context(harness.clone());
        dom.rebuild_in_place();
        dom.process_events();
        let mut editor = harness.editor.get().unwrap();
        let mut latest = harness.latest.get().unwrap();
        let original = published(5);
        let key = &original.key;
        dom.in_scope(ScopeId::APP, || {
            editor
                .drafts
                .write()
                .insert(key.clone(), TemplateDraft { revision: 5, body: "\"".repeat(50_000) });
            assert_eq!(
                editor.begin_save(&original),
                Some(MailTemplateBodyProblem::RequestTooLarge)
            );
            assert!(editor.confirmation.peek().is_none());
            assert!(harness.requests.borrow().is_empty());

            editor.drafts.write().get_mut(key).unwrap().body = "保留草稿".to_owned();
            assert_eq!(editor.begin_save(&original), None);
            let attempt = editor.confirmation.peek().clone().unwrap();
            editor.submit(attempt);
        });
        dom.process_events();
        assert!(editor.pending.contains(&template_entity(key)));
        let first: AdminMailTemplateUpdateRequest =
            serde_json::from_str(&harness.requests.borrow()[0]).unwrap();
        let command_id = Uuid::parse_str(&first.command_id).unwrap();

        dom.in_scope(ScopeId::APP, || latest.set(published(6)));
        for attempt in 2..=attempt_count {
            dom.in_scope(ScopeId::APP, || {
                consume_context::<PendingCommandHost>().retry(command_id);
            });
            dom.process_events();
            if attempt < attempt_count {
                assert!(editor.pending.contains(&template_entity(key)));
                assert!(!editor.conflicts.peek().contains(key));
                dom.in_scope(ScopeId::APP, || editor.discard(key, true));
                assert_eq!(editor.drafts.peek()[key].body, "保留草稿");
            }
        }
        assert!(!editor.pending.contains(&template_entity(key)));
        assert!(editor.conflicts.peek().contains(key));
        assert_eq!(editor.drafts.peek()[key].body, "保留草稿");
        {
            let requests = harness.requests.borrow();
            assert_eq!(requests.len(), attempt_count);
            assert!(requests.windows(2).all(|pair| pair[0] == pair[1]));
        }
        assert_eq!(first.expected_revision, 5);
        assert_eq!(first.body.as_deref(), Some("保留草稿"));

        dom.in_scope(ScopeId::APP, || editor.discard(key, true));
        dom.process_events();
        assert!(!editor.drafts.peek().contains_key(key));
        assert!(!editor.conflicts.peek().contains(key));
        dom.in_scope(ScopeId::APP, || {
            editor.drafts.write().insert(
                key.clone(),
                TemplateDraft { revision: 6, body: "以最新版本编辑".to_owned() },
            );
            assert_eq!(editor.begin_save(&published(6)), None);
            let attempt = editor.confirmation.peek().clone().unwrap();
            editor.submit(attempt);
        });
        dom.process_events();
        let requests = harness.requests.borrow();
        let new: AdminMailTemplateUpdateRequest =
            serde_json::from_str(&requests[attempt_count]).unwrap();
        assert_eq!(new.expected_revision, 6);
        assert_ne!(new.command_id, first.command_id);
    }
}
