//! 规则草稿编辑、校验和发布 workspace。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{AdminRules, AdminRulesCandidate, publish_admin_rules, validate_admin_rules};
use crate::app::surface::format_admin_time;

use super::helpers::{DEFAULT_RULES_LOCALE, draft_signature, notice_text, valid_locale};
use super::model::{RuleCommand, RuleNotice};
use super::preview::AdminRulesPreview;

/// 展示规则编辑器，并确保只有最近一次已校验草稿可以发布。
#[component]
pub(super) fn AdminRulesWorkspace(
    document: AdminRules,
    target: String,
    locales: Signal<Vec<String>>,
    new_locale: Signal<String>,
    target_signal: Signal<String>,
    draft: Signal<Option<AdminRulesCandidate>>,
    checked_signature: Signal<String>,
    command: Signal<Option<RuleCommand>>,
    notice: Signal<Option<RuleNotice>>,
    busy: Signal<bool>,
    reload_generation: Signal<u64>,
) -> Element {
    let current_draft = draft()
        .or_else(|| (document.revision == 0).then(|| super::helpers::to_draft(&document, &target)));
    let signature = current_draft.as_ref().map(draft_signature).unwrap_or_default();
    let base_draft = super::helpers::to_draft(&document, &target);
    let dirty = current_draft
        .as_ref()
        .is_some_and(|value| draft_signature(value) != draft_signature(&base_draft));
    let can_publish = !busy() && dirty && checked_signature() == signature;

    rsx! {
        Fragment {
            div {
                class: "admin-rules-tabs",
                role: "tablist",
                aria_label: t!("admin-rules-languages"),
                for locale in locales() {
                    button {
                        key: "{locale}",
                        r#type: "button",
                        role: "tab",
                        aria_selected: (locale == target).to_string(),
                        disabled: busy() && locale == target,
                        onclick: {
                            let current_target = target.clone();
                            let next_locale = locale.clone();
                            move |_| {
                                if next_locale.as_str() == current_target.as_str() {
                                    return;
                                }
                                draft.set(None);
                                checked_signature.set(String::new());
                                command.set(None);
                                notice.set(None);
                                target_signal.set(next_locale.clone());
                            }
                        },
                        "{locale}"
                    }
                }
            }

            form {
                class: "admin-rules-add",
                onsubmit: move |event| {
                    event.prevent_default();
                    let next_locale = new_locale().trim().to_owned();
                    if busy() || !valid_locale(&next_locale) {
                        return;
                    }
                    if !locales().iter().any(|locale| locale == &next_locale) {
                        let mut next_locales = locales();
                        next_locales.push(next_locale.clone());
                        locales.set(next_locales);
                    }
                    new_locale.set(String::new());
                    draft.set(None);
                    checked_signature.set(String::new());
                    command.set(None);
                    notice.set(None);
                    target_signal.set(next_locale);
                },
                label {
                    r#for: "rules-locale",
                    {t!("admin-rules-add-locale")}
                }
                input {
                    id: "rules-locale",
                    maxlength: "35",
                    placeholder: t!("admin-rules-locale-placeholder"),
                    required: true,
                    r#type: "text",
                    value: new_locale(),
                    disabled: busy(),
                    oninput: move |event| new_locale.set(event.value()),
                }
                button {
                    r#type: "submit",
                    disabled: busy() || !valid_locale(&new_locale()),
                    {t!("admin-rules-add")}
                }
            }

            label {
                class: "admin-rules-default",
                r#for: "rules-default-locale",
                {t!("admin-rules-default-locale")}
            }
            " "
            select {
                class: "admin-rules-default-locale",
                disabled: true,
                id: "rules-default-locale",
                value: DEFAULT_RULES_LOCALE,
                for locale in locales() {
                    option { key: "{locale}", value: "{locale}", "{locale}" }
                }
            }

            div { class: "admin-rules-facts",
                span {
                    {t!("admin-rules-revision", revision: document.revision)}
                }
                span {
                    {t!("admin-rules-published", actor: document.published_by.clone())}
                }
                span {
                    {t!("admin-rules-published-at")}
                    time {
                        datetime: document.published_at.clone(),
                        "{format_admin_time(&document.published_at)}"
                    }
                }
            }

            if let Some(value) = current_draft {
                form {
                    class: "admin-rules-form",
                    onsubmit: {
                        let candidate = value.clone();
                        move |event| {
                            event.prevent_default();
                            if busy() || candidate.title.trim().is_empty() {
                                return;
                            }
                            let mut busy = busy;
                            let mut draft = draft;
                            let mut checked_signature = checked_signature;
                            let mut command = command;
                            let mut notice = notice;
                            let locale = candidate.locale.clone();
                            let title = candidate.title.clone();
                            let markdown = candidate.markdown.clone();
                            busy.set(true);
                            notice.set(None);
                            spawn(async move {
                                match validate_admin_rules(locale, title, markdown).await {
                                    Ok(normalized) => {
                                        checked_signature.set(draft_signature(&normalized));
                                        draft.set(Some(normalized));
                                        command.set(None);
                                        notice.set(Some(RuleNotice::Validated));
                                    }
                                    Err(error) => {
                                        notice.set(Some(if error.is_conflict() {
                                            RuleNotice::Conflict
                                        } else {
                                            RuleNotice::ActionError(error.code)
                                        }));
                                    }
                                }
                                busy.set(false);
                            });
                        }
                    },
                    div { class: "admin-rules-editor",
                        div { class: "admin-rules-fields",
                            label {
                                r#for: "rules-title-input",
                                {t!("admin-rules-title")}
                            }
                            input {
                                id: "rules-title-input",
                                required: true,
                                r#type: "text",
                                value: value.title.clone(),
                                disabled: busy(),
                                oninput: {
                                    let input_value = value.clone();
                                    move |event| {
                                        let mut next = input_value.clone();
                                        next.title = event.value();
                                        draft.set(Some(next));
                                        checked_signature.set(String::new());
                                        command.set(None);
                                        notice.set(None);
                                    }
                                },
                            }
                            label {
                                r#for: "rules-markdown",
                                {t!("admin-rules-markdown")}
                            }
                            textarea {
                                id: "rules-markdown",
                                spellcheck: "false",
                                value: value.markdown.clone(),
                                disabled: busy(),
                                oninput: {
                                    let input_value = value.clone();
                                    move |event| {
                                        let mut next = input_value.clone();
                                        next.markdown = event.value();
                                        draft.set(Some(next));
                                        checked_signature.set(String::new());
                                        command.set(None);
                                        notice.set(None);
                                    }
                                },
                            }
                        }
                        AdminRulesPreview { draft: value.clone() }
                    }

                    if let Some(current_notice) = notice() {
                        p {
                            class: if matches!(
                                &current_notice,
                                RuleNotice::Validated | RuleNotice::Published
                            ) {
                                "admin-rules-ok"
                            } else {
                                "admin-rules-error"
                            },
                            role: if matches!(
                                &current_notice,
                                RuleNotice::Validated | RuleNotice::Published
                            ) { "status" } else { "alert" },
                            {notice_text(&current_notice)}
                        }
                    }

                    div { class: "admin-rules-actions",
                        button {
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| {
                                draft.set(None);
                                checked_signature.set(String::new());
                                command.set(None);
                                notice.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            },
                            {t!("admin-rules-reload")}
                        }
                        button {
                            r#type: "submit",
                            disabled: busy() || value.title.trim().is_empty(),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-rules-validate")} }
                        }
                        button {
                            r#type: "button",
                            disabled: !can_publish,
                            onclick: {
                                let publish_value = value.clone();
                                move |_| {
                                    let signature = draft_signature(&publish_value);
                                    let command_key = format!("{}:{signature}", document.revision);
                                    let current_command = match command() {
                                        Some(existing) if existing.signature == command_key => existing,
                                        _ => RuleCommand {
                                            command_id: Uuid::new_v4(),
                                            expected_revision: document.revision,
                                            draft: publish_value.clone(),
                                            signature: command_key,
                                        },
                                    };
                                    command.set(Some(current_command.clone()));
                                    let mut busy = busy;
                                    let mut notice = notice;
                                    let mut reload_generation = reload_generation;
                                    busy.set(true);
                                    notice.set(None);
                                    spawn(async move {
                                        let result = publish_admin_rules(
                                            current_command.command_id,
                                            current_command.expected_revision,
                                            current_command.draft.locale,
                                            current_command.draft.title,
                                            current_command.draft.markdown,
                                        )
                                        .await;
                                        match result {
                                            Ok(_) => {
                                                notice.set(Some(RuleNotice::Published));
                                                reload_generation.set(reload_generation().saturating_add(1));
                                            }
                                            Err(error) => {
                                                notice.set(Some(if error.is_conflict() {
                                                    RuleNotice::Conflict
                                                } else {
                                                    RuleNotice::ActionError(error.code)
                                                }));
                                            }
                                        }
                                        busy.set(false);
                                    });
                                }
                            },
                            if document.revision == 0 { {t!("admin-rules-create")} } else { {t!("admin-rules-publish")} }
                        }
                    }
                }
            }
        }
    }
}
