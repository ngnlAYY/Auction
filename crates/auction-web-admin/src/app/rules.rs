//! Admin 规则文档页面。
//!
//! 页面保留旧版的多语言编辑流程：先读取指定语言的安全快照，再编辑候选、调用服务端
//! 规范化校验，最后只允许发布最近一次已校验且仍然基于当前 revision 的草稿。

use auction_web_shared::{MarkdownDocument, SUPPORTED_LOCALES, without_leading_title};
use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    AdminRules, AdminRulesCandidate, fetch_admin_rules, publish_admin_rules, validate_admin_rules,
};

use super::{AdminPanel, format_admin_time};

const DEFAULT_RULES_LOCALE: &str = "en";

#[derive(Debug, Clone)]
struct RuleCommand {
    command_id: Uuid,
    expected_revision: i64,
    draft: AdminRulesCandidate,
    signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleNotice {
    Validated,
    Published,
    ActionError,
    Conflict,
}

/// 读取并展示 Admin 规则文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。
pub(super) fn page() -> Element {
    let target = use_signal(|| DEFAULT_RULES_LOCALE.to_owned());
    let locales = use_signal(|| {
        SUPPORTED_LOCALES.iter().map(|locale| locale.code.to_owned()).collect::<Vec<_>>()
    });
    let new_locale = use_signal(String::new);
    let reload_generation = use_signal(|| 0_u64);
    let mut draft = use_signal(|| None::<AdminRulesCandidate>);
    let mut checked_signature = use_signal(String::new);
    let mut command = use_signal(|| None::<RuleCommand>);
    let mut notice = use_signal(|| None::<RuleNotice>);
    let busy = use_signal(|| false);
    let rules = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_rules(target())
    });

    // 规则请求成功后才初始化编辑器，避免用户编辑过程中普通重绘把草稿覆盖掉。
    use_effect(move || {
        let requested_locale = target();
        let snapshot = rules.read_unchecked().clone();
        if let Some(Ok(document)) = snapshot {
            draft.set(Some(to_draft(&document, &requested_locale)));
            checked_signature.set(String::new());
            command.set(None);
            notice.set(None);
        }
    });

    match &*rules.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-rules-title"), detail: t!("admin-rules-loading") }
        },
        Some(Err(error)) if error.status == Some(404) => {
            let document = missing_rules_document(target());
            rsx! {
                section {
                    class: "admin-panel",
                    role: "status",
                    aria_labelledby: "admin-rules-missing-title",
                    h1 { id: "admin-rules-missing-title", {t!("page-rules-title")} }
                    p { class: "admin-panel__note", {t!("admin-rules-missing")} }
                }
                AdminRulesWorkspace {
                    document,
                    target: target(),
                    locales,
                    new_locale,
                    target_signal: target,
                    draft,
                    checked_signature,
                    command,
                    notice,
                    busy,
                    reload_generation,
                }
            }
        }
        Some(Err(_)) => rsx! {
            AdminRulesLoadError { reload_generation, missing: false }
        },
        Some(Ok(document)) => rsx! {
            AdminRulesWorkspace {
                document: document.clone(),
                target: target(),
                locales,
                new_locale,
                target_signal: target,
                draft,
                checked_signature,
                command,
                notice,
                busy,
                reload_generation,
            }
        },
    }
}

#[component]
fn AdminRulesLoadError(mut reload_generation: Signal<u64>, missing: bool) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-rules-error-title",
            h1 { id: "admin-rules-error-title", {t!("page-rules-title")} }
            p { class: "admin-panel__note", {
                if missing { t!("admin-rules-missing") } else { t!("admin-rules-error") }
            }}
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-rules-retry")}
            }
        }
    }
}

#[component]
fn AdminRulesWorkspace(
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
    let current_draft =
        draft().or_else(|| (document.revision == 0).then(|| to_draft(&document, &target)));
    let signature = current_draft.as_ref().map(draft_signature).unwrap_or_default();
    let base_draft = to_draft(&document, &target);
    let dirty = current_draft
        .as_ref()
        .is_some_and(|value| draft_signature(value) != draft_signature(&base_draft));
    let can_publish = !busy() && dirty && checked_signature() == signature;

    rsx! {
        section {
            class: "admin-panel admin-rules",
            aria_labelledby: "admin-rules-title",
            aria_busy: busy().to_string(),
            h1 { id: "admin-rules-title", {t!("page-rules-title")} }
            p { class: "admin-panel__note admin-rules__intro", {t!("admin-rules-description")} }

            div {
                class: "admin-rules__tabs",
                role: "tablist",
                aria_label: t!("admin-rules-languages"),
                for locale in locales() {
                    button {
                        key: "{locale}",
                        r#type: "button",
                        role: "tab",
                        aria_selected: (locale == target).to_string(),
                        disabled: busy() && locale == target,
                        class: if locale == target { "admin-rules__tab admin-rules__tab--active" } else { "admin-rules__tab" },
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

            div { class: "admin-rules__add-locale",
                label {
                    for: "admin-rules-new-locale",
                    {t!("admin-rules-add-locale")}
                }
                input {
                    id: "admin-rules-new-locale",
                    maxlength: "35",
                    placeholder: t!("admin-rules-locale-placeholder"),
                    value: new_locale(),
                    disabled: busy(),
                    oninput: move |event| new_locale.set(event.value()),
                }
                button {
                    r#type: "button",
                    disabled: busy() || !valid_locale(&new_locale()),
                    onclick: move |_| {
                        let next_locale = new_locale().trim().to_owned();
                        if !valid_locale(&next_locale) {
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
                    {t!("admin-rules-add")}
                }
            }

            div { class: "admin-rules__default",
                span { {t!("admin-rules-default-locale")} }
                code { "{DEFAULT_RULES_LOCALE}" }
            }

            div { class: "admin-rules__facts",
                span { {t!("admin-rules-revision", revision: document.revision)} }
                span { {t!("admin-rules-published", actor: document.published_by.clone())} }
                span {
                    {t!("admin-rules-published-at")}
                    time { datetime: document.published_at.clone(), "{format_admin_time(&document.published_at)}" }
                }
            }

            if let Some(value) = current_draft {
                form {
                    class: "admin-rules__form",
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
                                            RuleNotice::ActionError
                                        }));
                                    }
                                }
                                busy.set(false);
                            });
                        }
                    },
                div { class: "admin-rules__editor",
                    div { class: "admin-rules__fields",
                        label {
                            for: "admin-rules-title-input",
                            {t!("admin-rules-title")}
                        }
                        input {
                            id: "admin-rules-title-input",
                            required: true,
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
                            for: "admin-rules-markdown-input",
                            {t!("admin-rules-markdown")}
                        }
                        textarea {
                            id: "admin-rules-markdown-input",
                            rows: "20",
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
                        class: if matches!(current_notice, RuleNotice::Validated | RuleNotice::Published) {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if matches!(current_notice, RuleNotice::Validated | RuleNotice::Published) { "status" } else { "alert" },
                        {notice_text(current_notice)}
                    }
                }

                div { class: "admin-rules__actions",
                    button {
                        class: "admin-btn-secondary",
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
                        class: "admin-btn-secondary",
                        r#type: "submit",
                        disabled: busy() || value.title.trim().is_empty(),
                        if busy() { {t!("admin-action-working")} } else { {t!("admin-rules-validate")} }
                    }
                    button {
                        class: "admin-btn-primary",
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
                                                RuleNotice::ActionError
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

#[component]
fn AdminRulesPreview(draft: AdminRulesCandidate) -> Element {
    let markdown = without_leading_title(&draft.markdown);
    rsx! {
        article {
            class: "admin-rules__preview",
            aria_labelledby: "admin-rules-preview-title",
            h2 { id: "admin-rules-preview-title", {t!("admin-rules-preview")} }
            h1 {
                if draft.title.trim().is_empty() {
                    {t!("admin-rules-untitled")}
                } else {
                    "{draft.title}"
                }
            }
            if markdown.trim().is_empty() {
                p { class: "admin-rules__preview-empty", {t!("admin-rules-empty-preview")} }
            } else {
                MarkdownDocument { markdown }
            }
        }
    }
}

fn notice_text(notice: RuleNotice) -> String {
    match notice {
        RuleNotice::Validated => t!("admin-rules-validated"),
        RuleNotice::Published => t!("admin-rules-success"),
        RuleNotice::ActionError => t!("admin-rules-action-error"),
        RuleNotice::Conflict => t!("admin-rules-conflict"),
    }
}

fn to_draft(document: &AdminRules, requested_locale: &str) -> AdminRulesCandidate {
    AdminRulesCandidate {
        locale: requested_locale.to_owned(),
        title: document.title.clone(),
        markdown: document.markdown.clone(),
    }
}

fn missing_rules_document(locale: String) -> AdminRules {
    AdminRules {
        locale,
        title: String::new(),
        markdown: String::new(),
        published_by: t!("value-empty"),
        revision: 0,
        published_at: String::new(),
    }
}

fn draft_signature(draft: &AdminRulesCandidate) -> String {
    format!("{}\u{1f}{}\u{1f}{}", draft.locale, draft.title, draft.markdown)
}

fn valid_locale(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 35
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value.chars().all(|character| character.is_alphanumeric() || character == '-')
}
