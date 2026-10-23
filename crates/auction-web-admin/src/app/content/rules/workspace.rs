//! 规则草稿编辑、校验和发布 workspace。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::Time;

use crate::api::{AdminRules, AdminRulesCandidate, publish_admin_rules, validate_admin_rules};
use crate::app::surface::pending::{PendingCommands, set_if_alive};

use super::helpers::{DEFAULT_RULES_LOCALE, draft_signature, notice_text, valid_locale};
use super::model::RuleNotice;
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
    notice: Signal<Option<RuleNotice>>,
    busy: Signal<bool>,
    reload_generation: Signal<u64>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let frozen = move || busy() || pending.contains("rules");
    let current_draft = draft()
        .or_else(|| (document.revision == 0).then(|| super::helpers::to_draft(&document, &target)));
    let signature = current_draft.as_ref().map(draft_signature).unwrap_or_default();
    let base_draft = super::helpers::to_draft(&document, &target);
    let dirty = current_draft
        .as_ref()
        .is_some_and(|value| draft_signature(value) != draft_signature(&base_draft));
    let can_publish = !frozen() && dirty && checked_signature() == signature;

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
                        disabled: frozen(),
                        onclick: {
                            let current_target = target.clone();
                            let next_locale = locale.clone();
                            move |_| {
                                if frozen() || next_locale.as_str() == current_target.as_str() {
                                    return;
                                }
                                draft.set(None);
                                checked_signature.set(String::new());
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
                    if frozen() || !valid_locale(&next_locale) {
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
                    disabled: frozen(),
                    oninput: move |event| {
                        if !frozen() { new_locale.set(event.value()); }
                    },
                }
                button {
                    r#type: "submit",
                    disabled: frozen() || !valid_locale(&new_locale()),
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
                    Time { value: document.published_at.clone() }
                }
            }

            if let Some(value) = current_draft {
                form {
                    class: "admin-rules-form",
                    onsubmit: {
                        let candidate = value.clone();
                        move |event| {
                            event.prevent_default();
                            if frozen() || candidate.title.trim().is_empty() {
                                return;
                            }
                            let mut busy = busy;
                            let mut draft = draft;
                            let mut checked_signature = checked_signature;
                            let mut notice = notice;
                            let requested_signature = draft_signature(&candidate);
                            let locale = candidate.locale.clone();
                            let title = candidate.title.clone();
                            let markdown = candidate.markdown.clone();
                            busy.set(true);
                            notice.set(None);
                            dioxus::core::Runtime::current().in_scope(busy.origin_scope(), || {
                                spawn(async move {
                                let result = validate_admin_rules(locale.clone(), title, markdown).await;
                                // 校验只可回写发起时的语言与草稿，不能覆盖已切换的编辑目标。
                                if target_signal() != locale
                                    || draft().as_ref().is_some_and(|current| {
                                        draft_signature(current) != requested_signature
                                    })
                                {
                                    busy.set(false);
                                    return;
                                }
                                match result {
                                    Ok(normalized) => {
                                        checked_signature.set(draft_signature(&normalized));
                                        draft.set(Some(normalized));
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
                                disabled: frozen(),
                                oninput: {
                                    let input_value = value.clone();
                                    move |event| {
                                        if frozen() { return; }
                                        let mut next = input_value.clone();
                                        next.title = event.value();
                                        draft.set(Some(next));
                                        checked_signature.set(String::new());
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
                                disabled: frozen(),
                                oninput: {
                                    let input_value = value.clone();
                                    move |event| {
                                        if frozen() { return; }
                                        let mut next = input_value.clone();
                                        next.markdown = event.value();
                                        draft.set(Some(next));
                                        checked_signature.set(String::new());
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
                            disabled: frozen(),
                            onclick: move |_| {
                                if frozen() { return; }
                                draft.set(None);
                                checked_signature.set(String::new());
                                notice.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            },
                            {t!("admin-rules-reload")}
                        }
                        button {
                            r#type: "submit",
                            disabled: frozen() || value.title.trim().is_empty(),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-rules-validate")} }
                        }
                        button {
                            r#type: "button",
                            disabled: !can_publish,
                            onclick: {
                                let publish_value = value.clone();
                                move |_| {
                                    let signature = draft_signature(&publish_value);
                                    if frozen()
                                        || target_signal() != publish_value.locale
                                        || checked_signature() != signature
                                    {
                                        return;
                                    }
                                    let candidate = publish_value.clone();
                                    let revision = document.revision;
                                    notice.set(None);
                                    pending.submit(
                                        "rules".to_owned(),
                                        t!("admin-rules-publish"),
                                        move |command_id| {
                                            let candidate = candidate.clone();
                                            async move {
                                                publish_admin_rules(
                                                    command_id,
                                                    revision,
                                                    candidate.locale,
                                                    candidate.title,
                                                    candidate.markdown,
                                                )
                                                .await
                                                .map_err(|error| error.code)
                                            }
                                        },
                                        move |_| set_if_alive(notice, Some(RuleNotice::Published)),
                                        move |code| {
                                            set_if_alive(notice, Some(RuleNotice::ActionError(code)));
                                        },
                                    );
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
