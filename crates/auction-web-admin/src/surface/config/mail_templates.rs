//! 邮件模板卡片：出站通知正文保存在数据库，与 runtime 文件配置无关。逐个模板编辑，两阶段
//! 确认后保存或恢复内置默认正文；结果未知时只允许原样重试。

use std::collections::{BTreeMap, BTreeSet};

use auction_contracts::AdminMailTemplateDto;
use auction_web_shared::{AsyncState, AsyncStateKind, ConfirmCommand, t, use_confirm_command};
use dioxus::prelude::*;

use crate::api::{
    AdminApiFailure, MailTemplateAttempt, MailTemplateBodyProblem, fetch_mail_templates,
    mail_template_body_problem, submit_mail_template,
};
use crate::surface::{admin_api_error_message, format_admin_time};

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

/// 卡片持有的全部编辑状态。模板列表在刷新或读取失败时会卸载；草稿与结果未知的命令若留在
/// 各模板的编辑器里会随之丢失，重新挂载后只能发新命令，违反“结果未知只允许原样重试”。
/// 手写 `PartialEq` 只比较 Signal 身份（`#[component]` props 要求），不读值、不触发订阅。
#[derive(Clone, Copy)]
struct TemplateEditState {
    command: ConfirmCommand<MailTemplateAttempt, AdminApiFailure>,
    /// 正文与首次编辑时的版本一起保留，列表刷新不得替换草稿的版本围栏。
    drafts: Signal<BTreeMap<String, TemplateDraft>>,
    /// 服务端冲突按模板保留，避免另一模板的操作清除这一次拒绝。
    conflicts: Signal<BTreeSet<String>>,
    /// 最近一次被数据库接受的模板键，用于显示成功提示。
    accepted: Signal<Option<String>>,
    snapshot: Resource<Result<Vec<AdminMailTemplateDto>, String>>,
}

impl PartialEq for TemplateEditState {
    fn eq(&self, other: &Self) -> bool {
        self.command.command == other.command.command
            && self.command.confirmation == other.command.confirmation
            && self.command.submitting == other.command.submitting
            && self.command.failure == other.command.failure
            && self.command.uncertain == other.command.uncertain
            && self.drafts == other.drafts
            && self.conflicts == other.conflicts
            && self.accepted == other.accepted
            && self.snapshot == other.snapshot
    }
}

impl TemplateEditState {
    fn current_revision(self, key: &str) -> Option<i64> {
        let snapshot = self.snapshot.peek();
        let templates = snapshot.as_ref()?.as_ref().ok()?;
        templates.iter().find(|template| template.key == key).map(|template| template.revision)
    }

    fn can_edit(self, key: &str, revision: i64) -> bool {
        !*self.command.submitting.peek()
            && !*self.command.uncertain.peek()
            && self.command.confirmation.peek().is_none()
            && !self.conflicts.peek().contains(key)
            && self.current_revision(key) == Some(revision)
    }

    fn begin_save(mut self, template: &AdminMailTemplateDto) -> Option<MailTemplateBodyProblem> {
        let key = &template.key;
        if !self.can_edit(key, template.revision)
            || *self.snapshot.state().peek() != UseResourceState::Ready
        {
            return None;
        }
        let draft = self.drafts.peek().get(key).cloned()?;
        let revision = draft.save_revision(template.revision, false)?;
        if let Some(problem) = mail_template_body_problem(template, &draft.body) {
            return Some(problem);
        }
        self.accepted.set(None);
        // 签名覆盖全部意图字段：同一输入的重试复用 command_id，输入变化即生成新命令。
        let signature = format!("set:{key}:{revision}:{}", draft.body);
        self.command.begin(signature, |command_id| MailTemplateAttempt {
            command_id,
            key: key.clone(),
            expected_revision: revision,
            body: Some(draft.body),
        });
        None
    }

    /// 提交或原样重试；结果未知时 `ConfirmCommand` 只重放冻结的原命令。
    fn submit(mut self, pending: MailTemplateAttempt) {
        if !*self.command.uncertain.peek()
            && (self.command.confirmation.peek().as_ref() != Some(&pending)
                || self.conflicts.peek().contains(&pending.key)
                || *self.snapshot.state().peek() != UseResourceState::Ready
                || self.current_revision(&pending.key) != Some(pending.expected_revision))
        {
            return;
        }
        let key = pending.key.clone();
        let mut command = self.command;
        let mut accepted = self.accepted;
        let mut drafts = self.drafts;
        let mut conflicts = self.conflicts;
        let mut snapshot = self.snapshot;
        self.command.submit(
            pending,
            |attempt| async move { submit_mail_template(&attempt).await },
            AdminApiFailure::outcome_unknown,
            move |template: AdminMailTemplateDto| {
                drafts.write().remove(&template.key);
                accepted.set(Some(template.key));
                // 接受后的旧投影不能成为下一份草稿的基准。
                snapshot.clear();
                snapshot.restart();
            },
            move || {
                let conflict = command.failure.peek().as_ref().is_some_and(is_template_conflict);
                if conflict {
                    // 同 ID 账本检查和修订围栏证明旧命令不会再生效。结束确认状态，保留原基准和正文。
                    command.reset();
                    conflicts.write().insert(key);
                }
            },
        );
    }

    fn discard(mut self, key: &str, conflicted: bool) {
        if *self.command.submitting.peek() || *self.command.uncertain.peek() {
            return;
        }
        if self.command.command.peek().as_ref().is_some_and(|pending| pending.key == key) {
            self.command.reset();
        }
        self.drafts.write().remove(key);
        self.conflicts.write().remove(key);
        if conflicted {
            self.snapshot.clear();
            self.snapshot.restart();
        }
    }
}

fn is_template_conflict(failure: &AdminApiFailure) -> bool {
    failure.status == Some(409)
        && AdminApiFailure::display_code(&failure.code) == "mail_template_conflict"
        && AdminApiFailure::rejection_after_replay_check(&failure.code)
}

fn use_template_edit_state(
    snapshot: Resource<Result<Vec<AdminMailTemplateDto>, String>>,
) -> TemplateEditState {
    TemplateEditState {
        command: use_confirm_command(),
        drafts: use_signal(BTreeMap::new),
        conflicts: use_signal(BTreeSet::new),
        accepted: use_signal(|| None),
        snapshot,
    }
}

#[component]
pub(super) fn MailTemplateCard() -> Element {
    let mut snapshot = use_resource(fetch_mail_templates);
    let state = use_template_edit_state(snapshot);
    let uncertain = (state.command.uncertain)();
    let current = snapshot.read_unchecked().clone();
    rsx! {
        section { class: "admin-mail-templates admin-panel",
            h2 { {t!("admin-runtime-section-templates")} }
            p { class: "admin-panel__note", {t!("admin-mail-templates-note")} }
            button {
                class: "admin-btn-secondary",
                r#type: "button",
                onclick: move |_| snapshot.restart(),
                {t!("admin-mail-templates-refresh")}
            }
            // 结果未知的命令独立于列表显示：列表读取失败时仍保留原样重试入口。
            if uncertain {
                PendingTemplateRetry { state }
            }
            match current {
                None => rsx! {
                    AsyncState { kind: AsyncStateKind::Loading, title: t!("admin-mail-templates-loading"), detail: "" }
                },
                Some(Err(code)) => rsx! {
                    AsyncState { kind: AsyncStateKind::Error, title: super::super::admin_api_error_message(&code), detail: "" }
                },
                Some(Ok(templates)) => rsx! {
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

/// 结果未知时只显示原命令的重试；参数来自冻结的命令，不读取当前列表或草稿。
#[component]
fn PendingTemplateRetry(state: TemplateEditState) -> Element {
    let submitting = (state.command.submitting)();
    rsx! {
        if let Some(pending) = (state.command.command)() {
            p { class: "admin-panel__note", role: "status",
                {template_label(&pending.key)} ": " {t!("admin-mail-template-uncertain")}
            }
            button {
                class: "admin-btn-primary",
                r#type: "button",
                disabled: submitting,
                onclick: move |_| state.submit(pending.clone()),
                {t!("admin-mail-template-retry")}
            }
        }
    }
}

#[component]
fn MailTemplateEditor(template: AdminMailTemplateDto, state: TemplateEditState) -> Element {
    let TemplateEditState { mut command, mut drafts, mut accepted, .. } = state;
    let key = template.key.clone();
    let label = template_label(&key);
    let submitting = (command.submitting)();
    let uncertain = (command.uncertain)();
    // 同一时刻只有一条命令：其他模板的确认或重试进行中时，本模板的写入入口停用。
    let confirmation = (command.confirmation)();
    let own_confirmation = confirmation.clone().filter(|pending| pending.key == key);
    let confirming = own_confirmation.is_some();
    let busy = submitting || uncertain || confirmation.is_some();
    let list_ready = *state.snapshot.state().read() == UseResourceState::Ready;
    let failure = (command.failure)()
        .filter(|_| (command.command)().is_some_and(|pending| pending.key == key) && !uncertain);
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
    // 有草稿、待确认的命令、刚被接受的结果或失败提示时保持展开，否则这些内容会随分区折叠
    // 而看不到；其余时候由用户自己展开或折叠。
    let expanded = dirty || confirming || conflicted || just_accepted || failure.is_some();

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
            || *state.snapshot.state().peek() != UseResourceState::Ready
            || drafts.peek().get(&reset_key).is_some_and(|draft| draft.revision != revision)
        {
            return;
        }
        accepted.set(None);
        let signature = format!("reset:{reset_key}:{revision}");
        command.begin(signature, |command_id| MailTemplateAttempt {
            command_id,
            key: reset_key.clone(),
            expected_revision: revision,
            body: None,
        });
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
                if just_accepted {
                    p { class: "admin-shell__banner", role: "status", {t!("admin-mail-template-done")} }
                }
                if let Some(failure) = failure.filter(|failure| !is_template_conflict(failure)) {
                    p { class: "admin-shell__banner", role: "alert", {admin_api_error_message(AdminApiFailure::display_code(&failure.code))} }
                }
                if conflicted {
                    p { class: "admin-runtime-section-warning", role: "status", {t!("admin-mail-template-draft-conflict")} }
                    button {
                        class: "admin-btn-secondary",
                        r#type: "button",
                        disabled: submitting || uncertain || (busy && !confirming),
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
                            disabled: submitting || !list_ready,
                            onclick: move |_| state.submit(pending.clone()),
                            {t!("admin-mail-template-confirm")}
                        }
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: submitting,
                            onclick: move |_| command.cancel_confirmation(),
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

    use super::{
        TemplateDraft, TemplateEditState, is_template_conflict, placeholder_list,
        use_template_edit_state,
    };
    use crate::api::{AdminApiFailure, MailTemplateAttempt, MailTemplateBodyProblem};

    #[derive(Default)]
    struct EditorHarness {
        editor: Cell<Option<TemplateEditState>>,
        latest: Cell<Option<Signal<AdminMailTemplateDto>>>,
        requests: RefCell<Vec<String>>,
        failures: RefCell<VecDeque<AdminApiFailure>>,
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

    fn test_editor() -> Element {
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
        let snapshot = use_resource(move || {
            let template = latest();
            async move { Ok(vec![template]) }
        });
        harness.latest.set(Some(latest));
        harness.editor.set(Some(use_template_edit_state(snapshot)));
        rsx! { div {} }
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
        let rejected = AdminApiFailure { status: Some(409), code: "mail_template_conflict".into() };
        assert!(is_template_conflict(&rejected));
        assert_eq!(draft.save_revision(5, is_template_conflict(&rejected)), None);
        assert!(!is_template_conflict(&AdminApiFailure::client("network_error")));
        assert!(!is_template_conflict(&AdminApiFailure {
            status: Some(409),
            code: "http_409".into()
        }));
    }

    #[test]
    fn template_retries_send_the_frozen_revision_until_a_ledger_conflict_allows_reload() {
        let harness = Rc::new(EditorHarness::default());
        let attempts = [
            AdminApiFailure::client("request_timeout"),
            AdminApiFailure { status: Some(401), code: "unauthenticated".into() },
            AdminApiFailure { status: Some(429), code: "rate_limited".into() },
            AdminApiFailure::client("network_error"),
            AdminApiFailure::from_http_body(
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null}"#,
            ),
            AdminApiFailure::from_http_body(
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":7}"#,
            ),
            AdminApiFailure::from_http_body(
                409,
                r#"{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":null,"extra":true}"#,
            ),
            AdminApiFailure::from_http_body(
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
            assert!(editor.command.confirmation.peek().is_none());

            editor.drafts.write().get_mut(key).unwrap().body = "保留草稿".to_owned();
            assert_eq!(editor.begin_save(&original), None);
            let attempt = editor.command.confirmation.peek().clone().unwrap();
            editor.submit(attempt);
        });
        dom.process_events();
        assert!(*editor.command.uncertain.peek());

        dom.in_scope(ScopeId::APP, || latest.set(published(6)));
        for attempt in 2..=attempt_count {
            dom.in_scope(ScopeId::APP, || {
                let attempt = editor.command.command.peek().clone().unwrap();
                editor.submit(attempt);
            });
            dom.process_events();
            if attempt < attempt_count {
                assert!(*editor.command.uncertain.peek());
                assert!(!editor.conflicts.peek().contains(key));
                dom.in_scope(ScopeId::APP, || editor.discard(key, true));
                assert_eq!(editor.drafts.peek()[key].body, "保留草稿");
            }
        }
        assert!(!*editor.command.uncertain.peek());
        assert!(editor.conflicts.peek().contains(key));
        assert_eq!(editor.drafts.peek()[key].body, "保留草稿");
        let first = {
            let requests = harness.requests.borrow();
            assert_eq!(requests.len(), attempt_count);
            assert!(requests.windows(2).all(|pair| pair[0] == pair[1]));
            serde_json::from_str::<AdminMailTemplateUpdateRequest>(&requests[0]).unwrap()
        };
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
            let attempt = editor.command.confirmation.peek().clone().unwrap();
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
