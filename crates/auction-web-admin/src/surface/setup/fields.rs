//! 首次部署的字段控件和可定位错误摘要；标签、帮助与错误全部使用当前语言。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::surface::validation::{FieldError, focus_field};

use super::{
    draft::{SetupDraft, SetupField},
    validation::SetupIssue,
};

#[component]
pub(super) fn SetupInput(
    field: SetupField,
    mut draft: Signal<SetupDraft>,
    mut issues: Signal<Vec<SetupIssue>>,
    busy: bool,
) -> Element {
    let id = field.id();
    let mut reveal_secret = use_signal(|| false);
    let help_id = format!("{id}-help");
    let error_id = format!("{id}-error");
    let error = issues.read().iter().find(|issue| issue.field == field).map(|issue| t!(issue.key));
    let invalid = error.is_some();
    let description = if invalid { format!("{help_id} {error_id}") } else { help_id.clone() };
    let bounds = field.number_bounds();

    rsx! {
        div { class: "admin-setup__field",
            label { r#for: id, {t!(id)} }
            if field.is_weekday() {
                select {
                    id,
                    name: id,
                    autocomplete: "off",
                    required: true,
                    disabled: busy,
                    aria_invalid: invalid.to_string(),
                    aria_describedby: description,
                    value: draft.read().value(field).to_owned(),
                    oninput: move |event| {
                        if busy { return; }
                        draft.write().set(field, event.value());
                        issues.write().retain(|issue| !field.related(issue.field));
                    },
                    for (value, label) in SetupField::WEEKDAYS {
                        option { value, {t!(label)} }
                    }
                }
            } else {
                input {
                    id,
                    name: id,
                    r#type: if field == SetupField::AdminSecret && reveal_secret() { "text" } else { field.input_type() },
                    inputmode: if bounds.is_some() { "numeric" } else if field.input_type() == "url" { "url" } else { "text" },
                    autocomplete: field.autocomplete(),
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    required: true,
                    disabled: busy,
                    min: bounds.map(|(minimum, _, _)| minimum.to_string()),
                    max: bounds.map(|(_, maximum, _)| maximum.to_string()),
                    step: if field.is_schedule_time() { Some("60".to_owned()) } else { bounds.map(|(_, _, step)| step.to_string()) },
                    maxlength: field.maximum_bytes().map(|maximum| maximum as i64),
                    aria_invalid: invalid.to_string(),
                    aria_describedby: description,
                    value: draft.read().value(field).to_owned(),
                    oninput: move |event| {
                        if busy { return; }
                        draft.write().set(field, event.value());
                        issues.write().retain(|issue| !field.related(issue.field));
                    },
                }
            }
            if field == SetupField::AdminSecret {
                div { class: "admin-setup__actions",
                    button {
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| {
                            if busy { return; }
                            draft.write().generate_admin_secret();
                            issues.write().retain(|issue| !field.related(issue.field));
                            reveal_secret.set(true);
                        },
                        {t!("setup-admin-secret-generate")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy,
                        aria_controls: id,
                        aria_pressed: reveal_secret().to_string(),
                        onclick: move |_| { if !busy { reveal_secret.toggle(); } },
                        if reveal_secret() { {t!("setup-admin-secret-hide")} }
                        else { {t!("setup-admin-secret-show")} }
                    }
                }
            }
            p { id: help_id, class: "admin-setup__hint", {t!(&format!("{id}-help"))} }
            FieldError { id: error_id, message: error }
        }
    }
}

#[component]
pub(super) fn SetupErrors(
    issues: Signal<Vec<SetupIssue>>,
    mut advanced_open: Signal<bool>,
) -> Element {
    if issues.read().is_empty() {
        return rsx! {};
    }
    rsx! {
        section {
            class: "admin-setup__errors",
            role: "alert",
            aria_labelledby: "setup-errors-title",
            h2 { id: "setup-errors-title", {t!("setup-errors-title")} }
            ul {
                for issue in issues() {
                    li { key: "{issue.field.id()}",
                        a {
                            href: "#{issue.field.id()}",
                            onclick: move |event| {
                                event.prevent_default();
                                if SetupField::ADVANCED.contains(&issue.field) {
                                    advanced_open.set(true);
                                }
                                focus_field(issue.field.id());
                            },
                            {t!(issue.field.id())}
                        }
                    }
                }
            }
        }
    }
}
