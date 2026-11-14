//! 邮件模板卡片：出站通知正文保存在数据库，与 runtime 文件配置无关。逐个模板编辑，两阶段
//! 确认后保存或恢复内置默认正文；结果未知时只允许原样重试。

use std::collections::BTreeMap;

use crate::api::{
    AdminApiFailure, MailTemplateAttempt, MailTemplateBodyProblem, fetch_mail_templates,
    mail_template_body_problem, mail_template_outcome_unknown, submit_mail_template,
};
use auction_contracts::AdminMailTemplateDto;
use auction_web_shared::{
    AsyncState, AsyncStateKind, ConfirmCommand, legacy_t as t, use_confirm_command,
};
use dioxus::prelude::*;

/// 卡片持有的全部编辑状态。模板列表在刷新或读取失败时会卸载；草稿与结果未知的命令若留在
/// 各模板的编辑器里会随之丢失，重新挂载后只能发新命令，违反“结果未知只允许原样重试”。
/// 手写 `PartialEq` 只比较 Signal 身份（`#[component]` props 要求），不读值、不触发订阅。
#[derive(Clone, Copy)]
struct TemplateEditState {
    command: ConfirmCommand<MailTemplateAttempt, AdminApiFailure>,
    /// 尚未保存的正文，按模板键保存；与已保存正文相同时不保留。
    drafts: Signal<BTreeMap<String, String>>,
    /// 最近一次被数据库接受的模板键，用于显示成功提示。
    accepted: Signal<Option<String>>,
    refresh: Signal<u64>,
}

impl PartialEq for TemplateEditState {
    fn eq(&self, other: &Self) -> bool {
        self.command.command == other.command.command
            && self.command.confirmation == other.command.confirmation
            && self.command.submitting == other.command.submitting
            && self.command.failure == other.command.failure
            && self.command.uncertain == other.command.uncertain
            && self.drafts == other.drafts
            && self.accepted == other.accepted
            && self.refresh == other.refresh
    }
}

impl TemplateEditState {
    /// 提交或原样重试；结果未知时 `ConfirmCommand` 只重放冻结的原命令。
    fn submit(mut self, pending: MailTemplateAttempt) {
        let mut accepted = self.accepted;
        let mut drafts = self.drafts;
        let mut refresh = self.refresh;
        self.command.submit(
            pending,
            |attempt| async move { submit_mail_template(&attempt).await },
            |failure: &AdminApiFailure| mail_template_outcome_unknown(failure),
            move |template: AdminMailTemplateDto| {
                drafts.write().remove(&template.key);
                accepted.set(Some(template.key));
                refresh.set(refresh().saturating_add(1));
            },
            || {},
        );
    }
}

#[component]
pub(super) fn MailTemplateCard() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let state = TemplateEditState {
        command: use_confirm_command(),
        drafts: use_signal(BTreeMap::new),
        accepted: use_signal(|| None),
        refresh,
    };
    let uncertain = (state.command.uncertain)();
    let snapshot = use_resource(move || {
        let _ = refresh();
        fetch_mail_templates()
    });
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
    let failure = (command.failure)()
        .filter(|_| (command.command)().is_some_and(|pending| pending.key == key) && !uncertain);
    let draft = drafts.read().get(&key).cloned();
    let dirty = draft.is_some();
    let body = draft.unwrap_or_else(|| template.body.clone());
    let problem = mail_template_body_problem(&template, &body);
    let just_accepted = accepted().as_deref() == Some(key.as_str());
    // 有草稿、待确认的命令、刚被接受的结果或失败提示时保持展开，否则这些内容会随分区折叠
    // 而看不到；其余时候由用户自己展开或折叠。
    let expanded = dirty || confirming || just_accepted || failure.is_some();

    let saved_body = template.body.clone();
    let input_key = key.clone();
    let edit = move |event: FormEvent| {
        let value = event.value();
        accepted.set(None);
        let mut drafts = drafts.write();
        // 改回已保存的正文即没有草稿，保存按钮随之停用。
        if value == saved_body {
            drafts.remove(&input_key);
        } else {
            drafts.insert(input_key.clone(), value);
        }
    };

    let save_key = key.clone();
    let save_body = body.clone();
    let revision = template.revision;
    let begin_save = move |_| {
        accepted.set(None);
        // 签名覆盖全部意图字段：同一输入的重试复用 command_id，输入变化即生成新命令。
        let signature = format!("set:{save_key}:{revision}:{save_body}");
        command.begin(signature, |command_id| MailTemplateAttempt {
            command_id,
            key: save_key.clone(),
            expected_revision: revision,
            body: Some(save_body.clone()),
        });
    };
    let reset_key = key.clone();
    let begin_reset = move |_| {
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
        drafts.write().remove(&discard_key);
    };

    let status = if !template.customized {
        t!("admin-mail-template-status-default")
    } else if template.usable {
        t!("admin-mail-template-status-custom")
    } else {
        t!("admin-mail-template-status-unusable")
    };
    let optional_placeholders: Vec<_> = template
        .allowed_placeholders
        .iter()
        .filter(|name| !template.required_placeholders.contains(name))
        .cloned()
        .collect();

    rsx! {
        details { class: "admin-runtime-section admin-mail-template", open: expanded,
            summary {
                span { "{label}" }
                span { class: "admin-mail-template__status", "{status}" }
            }
            div { class: "admin-runtime-section__body",
                if let (Some(actor), Some(time)) = (template.updated_by.clone(), template.updated_at) {
                    p { class: "admin-runtime-section-note",
                        {t!("admin-mail-template-revision", revision: template.revision, actor: actor, time: time.to_rfc3339())}
                    }
                }
                p { class: "admin-runtime-section-note",
                    if template.allowed_placeholders.is_empty() {
                        {t!("admin-mail-template-no-placeholders")}
                    } else {
                        {t!("admin-mail-template-required")} ": " {placeholder_list(&template.required_placeholders)}
                        " · "
                        {t!("admin-mail-template-optional")} ": " {placeholder_list(&optional_placeholders)}
                    }
                }
                label { class: "admin-runtime-field admin-runtime-field--template",
                    span { "{label}" }
                    textarea {
                        name: "mail-template-{key}",
                        autocomplete: "off",
                        spellcheck: "false",
                        rows: "8",
                        disabled: busy,
                        value: body,
                        oninput: edit,
                    }
                }
                if let Some(problem) = problem.filter(|_| dirty) {
                    p { class: "admin-runtime-section-warning", role: "status", {problem_message(problem)} }
                }
                if just_accepted {
                    p { class: "admin-shell__banner", role: "status", {t!("admin-mail-template-done")} }
                }
                if let Some(failure) = failure {
                    p { class: "admin-shell__banner", role: "alert", {super::super::admin_api_error_message(&failure.code)} }
                }
                if let Some(pending) = own_confirmation {
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
                            disabled: submitting,
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
                            r#type: "button",
                            disabled: busy || !dirty || problem.is_some(),
                            onclick: begin_save,
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
                            disabled: busy || !template.customized,
                            onclick: begin_reset,
                            {t!("admin-mail-template-reset")}
                        }
                    }
                }
            }
        }
    }
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
    use super::placeholder_list;

    #[test]
    fn placeholders_are_shown_the_way_they_are_written_in_the_body() {
        assert_eq!(
            placeholder_list(&["amount".to_owned(), "reference".to_owned()]),
            "{amount} {reference}"
        );
        assert_eq!(placeholder_list(&[]), "—");
    }
}
