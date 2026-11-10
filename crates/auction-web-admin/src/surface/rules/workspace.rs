//! 规则草稿编辑、校验和发布 workspace。

use auction_web_shared::{Dialog, Time, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{AdminRules, AdminRulesCandidate, publish_admin_rules, validate_admin_rules};
use crate::surface::pending::{PendingCommands, set_if_alive};

use super::helpers::{DEFAULT_RULES_LOCALE, notice_text, valid_locale};
use super::model::{RuleNotice, RulesBaseline, RulesTransition};
use super::preview::AdminRulesPreview;

/// 展示规则编辑器，并确保只有最近一次已校验草稿可以发布。
#[component]
pub(super) fn AdminRulesWorkspace(
    document: AdminRules,
    target: String,
    mut locales: Signal<Vec<String>>,
    mut new_locale: Signal<String>,
    mut target_signal: Signal<String>,
    mut draft: Signal<Option<AdminRulesCandidate>>,
    mut checked_candidate: Signal<Option<AdminRulesCandidate>>,
    mut baseline: Signal<Option<RulesBaseline>>,
    mut notice: Signal<Option<RuleNotice>>,
    busy: Signal<bool>,
    mut reload_generation: Signal<u64>,
    pending: PendingCommands,
    dirty: Memo<bool>,
) -> Element {
    let frozen = move || busy() || pending.contains("rules");
    let mut transition_confirmation = use_signal(|| None::<RulesTransition>);
    let current_draft = draft()
        .or_else(|| (document.revision == 0).then(|| super::helpers::to_draft(&document, &target)));
    let conflict = baseline.read().as_ref().is_none_or(|base| !base.matches_document(&document));
    let can_publish = !frozen()
        && !conflict
        && dirty()
        && checked_candidate.read().as_ref() == current_draft.as_ref();

    // 修改目标或重载只在用户确认丢弃后执行；校验和已提交命令仍共用原门禁。
    let apply_transition = use_callback(move |transition: RulesTransition| {
        if frozen() {
            return;
        }
        match transition {
            RulesTransition::Select { locale, add } => {
                if locale == *target_signal.peek() {
                    if add {
                        new_locale.set(String::new());
                    }
                    transition_confirmation.set(None);
                    return;
                }
                if add {
                    if !locales.peek().iter().any(|existing| existing == &locale) {
                        locales.write().push(locale.clone());
                    }
                    new_locale.set(String::new());
                }
                draft.set(None);
                baseline.set(None);
                checked_candidate.set(None);
                notice.set(None);
                transition_confirmation.set(None);
                target_signal.set(locale);
            }
            RulesTransition::Reload => {
                draft.set(None);
                baseline.set(None);
                checked_candidate.set(None);
                notice.set(None);
                transition_confirmation.set(None);
                reload_generation.set(reload_generation().saturating_add(1));
            }
        }
    });
    let request_transition = use_callback(move |transition: RulesTransition| {
        if frozen() {
            return;
        }
        // 添加当前语言不切换目标，也不能清空当前草稿。
        let same_target = match &transition {
            RulesTransition::Select { locale, .. } => locale == &*target_signal.peek(),
            RulesTransition::Reload => false,
        };
        if same_target {
            apply_transition.call(transition);
        } else if match (draft.peek().as_ref(), baseline.peek().as_ref()) {
            (Some(candidate), Some(base)) => !base.matches_candidate(candidate),
            (Some(_), None) => true,
            (None, _) => false,
        } {
            transition_confirmation.set(Some(transition));
        } else {
            apply_transition.call(transition);
        }
    });

    rsx! {
        Fragment {
            div {
                class: "admin-rules-tabs",
                role: "group",
                aria_label: t!("admin-rules-languages"),
                for locale in locales() {
                    button {
                        key: "{locale}",
                        r#type: "button",
                        aria_pressed: (locale == target).to_string(),
                        disabled: frozen(),
                        onclick: {
                            let next_locale = locale.clone();
                            move |_| request_transition.call(RulesTransition::Select {
                                locale: next_locale.clone(),
                                add: false,
                            })
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
                    request_transition.call(RulesTransition::Select { locale: next_locale, add: true });
                },
                label {
                    r#for: "rules-locale",
                    {t!("admin-rules-add-locale")}
                }
                input {
                    id: "rules-locale",
                    name: "locale",
                    autocomplete: "off",
                    autocapitalize: "none",
                    spellcheck: "false",
                    maxlength: "35",
                    aria_describedby: "rules-locale-help",
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
                p { id: "rules-locale-help", class: "admin-panel__note admin-rules-add__help", {t!("admin-rules-locale-help")} }
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
                name: "default_locale",
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
            if conflict {
                p { class: "admin-rules-error", role: "alert", {t!("admin-rules-draft-conflict")} }
            }

            if let Some(value) = current_draft {
                form {
                    class: "admin-rules-form",
                    onsubmit: {
                        let candidate = value.clone();
                        move |event| {
                            event.prevent_default();
                            if frozen() || conflict || candidate.title.trim().is_empty()
                                || draft.peek().as_ref() != Some(&candidate)
                            {
                                return;
                            }
                            let mut busy = busy;
                            let mut draft = draft;
                            let mut checked_candidate = checked_candidate;
                            let mut notice = notice;
                            let requested_candidate = candidate.clone();
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
                                    || draft.peek().as_ref() != Some(&requested_candidate)
                                {
                                    busy.set(false);
                                    return;
                                }
                                match result {
                                    Ok(normalized) => {
                                        checked_candidate.set(Some(normalized.clone()));
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
                                name: "title",
                                autocomplete: "off",
                                required: true,
                                r#type: "text",
                                value: value.title.clone(),
                                disabled: frozen(),
                                oninput: {
                                    let input_locale = target.clone();
                                    move |event| {
                                        if frozen() || *target_signal.peek() != input_locale { return; }
                                        let mut current = draft.write();
                                        let candidate = current.get_or_insert_with(|| AdminRulesCandidate {
                                            locale: input_locale.clone(),
                                            title: String::new(),
                                            markdown: String::new(),
                                        });
                                        candidate.title = event.value();
                                        drop(current);
                                        checked_candidate.set(None);
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
                                name: "markdown",
                                autocomplete: "off",
                                spellcheck: "false",
                                value: value.markdown.clone(),
                                disabled: frozen(),
                                oninput: {
                                    let input_locale = target.clone();
                                    move |event| {
                                        if frozen() || *target_signal.peek() != input_locale { return; }
                                        let mut current = draft.write();
                                        let candidate = current.get_or_insert_with(|| AdminRulesCandidate {
                                            locale: input_locale.clone(),
                                            title: String::new(),
                                            markdown: String::new(),
                                        });
                                        candidate.markdown = event.value();
                                        drop(current);
                                        checked_candidate.set(None);
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
                            onclick: move |_| request_transition.call(RulesTransition::Reload),
                            {t!("admin-rules-reload")}
                        }
                        button {
                            r#type: "submit",
                            disabled: frozen() || conflict || value.title.trim().is_empty(),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-rules-validate")} }
                        }
                        button {
                            r#type: "button",
                            disabled: !can_publish,
                            onclick: {
                                let publish_value = value.clone();
                                move |_| {
                                    if frozen()
                                        || conflict
                                        || target_signal() != publish_value.locale
                                        || checked_candidate.peek().as_ref() != Some(&publish_value)
                                        || draft.peek().as_ref() != Some(&publish_value)
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
                                        move |_| {
                                            set_if_alive(draft, None);
                                            set_if_alive(baseline, None);
                                            set_if_alive(checked_candidate, None);
                                            set_if_alive(notice, Some(RuleNotice::Published));
                                        },
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
            if let Some(transition) = transition_confirmation() {
                Dialog {
                    title: t!("admin-rules-discard-title"),
                    close_label: t!("admin-runtime-keep-editing"),
                    on_close: move |_| transition_confirmation.set(None),
                    match &transition {
                        RulesTransition::Select { locale, .. } => rsx! {
                            p { {t!("admin-rules-discard-switch-detail", locale: locale.clone())} }
                        },
                        RulesTransition::Reload => rsx! {
                            p { {t!("admin-rules-discard-reload-detail")} }
                        },
                    }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            autofocus: true,
                            onclick: move |_| transition_confirmation.set(None),
                            {t!("admin-runtime-keep-editing")}
                        }
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: frozen(),
                            onclick: move |_| apply_transition.call(transition.clone()),
                            {t!("admin-rules-discard-confirm")}
                        }
                    }
                }
            }
        }
    }
}
