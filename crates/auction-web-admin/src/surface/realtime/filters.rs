//! Realtime 运维页面的筛选器与字段反馈。
//! 草稿留在表单内，只有通过校验的筛选才能提交到地址栏。

use auction_web_shared::{a11y::focus_element_by_id_after_render, t};
use dioxus::prelude::*;

use super::filter_field::{FilterField, FilterValue};

pub(super) const EVENT_FILTER_FORM_ID: &str = "realtime-event-filters";
pub(super) const FAILURE_FILTER_FORM_ID: &str = "realtime-failure-filters";

/// 申请 realtime 事件元数据查询筛选条件。
#[component]
pub(super) fn RealtimeEventFilters(
    applied: Memo<(FilterValue, FilterValue)>,
    invalid_applied: bool,
    on_load: EventHandler<(String, String)>,
) -> Element {
    let submitted = use_signal(|| false);
    let mut draft = use_filter_draft(applied, submitted);
    let show_errors = invalid_applied || submitted();
    rsx! {
        form {
            id: EVENT_FILTER_FORM_ID,
            class: "admin-realtime-filter",
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                let draft = draft.read();
                let fields = [
                    (FilterField::EventStream, &draft.0),
                    (FilterField::EventKind, &draft.1),
                ];
                if let Some([stream, kind]) = validate_fields(fields, submitted) {
                    on_load.call((stream.to_owned(), kind.to_owned()));
                }
            },
            FilterInput {
                field: FilterField::EventStream,
                name: "stream",
                label: t!("realtime-filter-stream"),
                value: draft.read().0.clone(),
                show_errors,
                on_input: move |value| draft.write().0 = FilterValue::Text(value),
            }
            FilterInput {
                field: FilterField::EventKind,
                name: "kind",
                label: t!("realtime-filter-kind"),
                value: draft.read().1.clone(),
                show_errors,
                on_input: move |value| draft.write().1 = FilterValue::Text(value),
            }
            button { r#type: "submit", {t!("realtime-filter-apply")} }
        }
    }
}

/// 申请 realtime failure 查询筛选条件。
#[component]
pub(super) fn RealtimeFailureFilters(
    applied: Memo<(FilterValue, FilterValue, FilterValue, FilterValue)>,
    invalid_applied: bool,
    on_load: EventHandler<(String, String, String)>,
) -> Element {
    let submitted = use_signal(|| false);
    let mut draft = use_filter_draft(applied, submitted);
    let show_errors = invalid_applied || submitted();
    rsx! {
        details { class: "admin-realtime-disclosure", open: invalid_applied,
            summary { {t!("realtime-failure-filters")} }
            form {
                id: FAILURE_FILTER_FORM_ID,
                class: "admin-realtime-filter",
                autocomplete: "off",
                onsubmit: move |event| {
                    event.prevent_default();
                    let draft = draft.read();
                    let fields = [
                        (FilterField::FailureStream, &draft.0),
                        (FilterField::FailureConsumer, &draft.1),
                        (FilterField::FailureCode, &draft.2),
                        (FilterField::FailureState, &draft.3),
                    ];
                    if let Some([stream, consumer, code, _]) = validate_fields(fields, submitted) {
                        on_load.call((stream.to_owned(), consumer.to_owned(), code.to_owned()));
                    }
                },
                FilterInput {
                    field: FilterField::FailureStream,
                    name: "stream",
                    label: t!("realtime-filter-stream"),
                    value: draft.read().0.clone(),
                    show_errors,
                    on_input: move |value| draft.write().0 = FilterValue::Text(value),
                }
                FilterInput {
                    field: FilterField::FailureConsumer,
                    name: "consumer",
                    label: t!("realtime-filter-consumer"),
                    value: draft.read().1.clone(),
                    show_errors,
                    on_input: move |value| draft.write().1 = FilterValue::Text(value),
                }
                FilterInput {
                    field: FilterField::FailureCode,
                    name: "code",
                    label: t!("realtime-filter-code"),
                    value: draft.read().2.clone(),
                    show_errors,
                    on_input: move |value| draft.write().2 = FilterValue::Text(value),
                }
                button { r#type: "submit", {t!("realtime-filter-apply")} }
            }
        }
    }
}

/// 状态选择沿用立即应用的交互，保留非法历史值并允许从正常选项中修正。
#[component]
pub(super) fn RealtimeFailureStateFilter(
    value: FilterValue,
    on_change: EventHandler<String>,
) -> Element {
    let field = FilterField::FailureState;
    rsx! {
        div { class: "admin-realtime-status",
            QueryFilterSelect {
                id: field.input_id(),
                name: "failure_state",
                label: t!("realtime-filter-state"),
                value,
                choices: vec![
                    ("", t!("realtime-filter-all"), false),
                    ("pending", t!("realtime-state-pending"), true),
                    ("retry", t!("realtime-state-retry"), true),
                    ("dead_letter", t!("realtime-state-dead-letter"), false),
                    ("completed", t!("realtime-state-completed"), false),
                ],
                on_change: move |value: String| {
                    if let Some(normalized) = field.normalize(&value) {
                        on_change.call(normalized.to_owned());
                    }
                },
            }
        }
    }
}

#[component]
fn FilterInput(
    field: FilterField,
    name: &'static str,
    label: String,
    value: FilterValue,
    show_errors: bool,
    on_input: EventHandler<String>,
) -> Element {
    let invalid_draft =
        show_errors && value.text().is_some_and(|text| field.normalize(text).is_none());
    rsx! {
        QueryFilterInput {
            id: field.input_id(),
            name, label, on_input,
            input_type: "search",
            value: value.input_value().to_owned(),
            error_values: value.error_values(invalid_draft),
        }
    }
}

/// 单行或本地时间筛选框，关联字段错误与转义后的地址原值。
#[component]
pub(in crate::surface) fn QueryFilterInput(
    id: &'static str,
    name: &'static str,
    label: String,
    input_type: &'static str,
    value: String,
    error_values: Vec<String>,
    on_input: EventHandler<String>,
) -> Element {
    let error_id = format!("{id}-error");
    let label_id = format!("{id}-label");
    let invalid = !error_values.is_empty();
    rsx! {
        label { r#for: id,
            span { id: "{label_id}", "{label}" }
            input {
                id, name, value,
                r#type: input_type,
                autocapitalize: "none",
                spellcheck: "false",
                enterkeyhint: "search",
                aria_labelledby: label_id,
                aria_invalid: invalid.to_string(),
                aria_describedby: invalid.then(|| error_id.clone()),
                oninput: move |event| on_input.call(event.value()),
            }
            if invalid {
                {filter_error(&error_id, &error_values, input_type == "datetime-local")}
            }
        }
    }
}

/// 闭集筛选显示无效地址，并等待用户从允许的选项中选择。
#[component]
pub(in crate::surface) fn QueryFilterSelect(
    id: &'static str,
    name: &'static str,
    label: String,
    value: FilterValue,
    choices: Vec<(&'static str, String, bool)>,
    on_change: EventHandler<String>,
) -> Element {
    const INVALID_OPTION: &str = "invalid";
    let selected = value.text();
    let error_values = value.error_values(false);
    let invalid = !error_values.is_empty();
    let error_id = format!("{id}-error");
    let label_id = format!("{id}-label");
    rsx! {
        label { r#for: id,
            span { id: "{label_id}", "{label}" }
            select {
                id, name,
                aria_labelledby: label_id,
                aria_invalid: invalid.to_string(),
                aria_describedby: invalid.then(|| error_id.clone()),
                onchange: move |event| on_change.call(event.value()),
                if invalid {
                    option { value: INVALID_OPTION, selected: true, disabled: true, {t!("value-empty")} }
                }
                for (choice, label, disabled) in choices {
                    // 动态选项在 select 之后挂载，选中状态随选项本身应用。
                    option { key: "{choice}", value: choice, selected: selected == Some(choice), disabled, "{label}" }
                }
            }
            if invalid {
                {filter_error(&error_id, &error_values, false)}
            }
        }
    }
}

fn filter_error(id: &str, originals: &[String], is_local_time: bool) -> Element {
    let message = if is_local_time && originals.len() == 1 {
        t!("admin-audit-invalid-time")
    } else {
        t!("admin-error-invalid-request")
    };
    let originals = originals.iter().enumerate().map(|(index, value)| {
        let controls = value
            .chars()
            .filter(|character| character.is_control())
            .map(|character| format!("U+{:04X}", u32::from(character)))
            .collect::<Vec<_>>()
            .join(", ");
        (index, format!("{value:?}"), controls)
    });
    rsx! {
        span { id: "{id}", class: "action-feedback action-feedback--error", role: "alert",
            "{message}"
            for (index, escaped, controls) in originals {
                span { key: "{index}",
                    " "
                    code {
                        for (part_index, part) in escaped_parts(&escaped).enumerate() {
                            if part_index > 0 { wbr {} }
                            "{part}"
                        }
                    }
                    if !controls.is_empty() {
                        strong { " ({controls})" }
                    }
                }
            }
        }
    }
}

fn escaped_parts(escaped: &str) -> impl Iterator<Item = &str> {
    let mut run = 0;
    // 只补连续 ASCII 的折行机会，连接各段仍得到完整的转义值。
    escaped.split_inclusive(move |character: char| {
        if !character.is_ascii_graphic() {
            run = 0;
            return false;
        }
        run += 1;
        if run < 16 {
            return false;
        }
        run = 0;
        true
    })
}

fn validate_fields<const N: usize>(
    fields: [(FilterField, &FilterValue); N],
    mut submitted: Signal<bool>,
) -> Option<[&str; N]> {
    let mut values = [""; N];
    for (index, (field, value)) in fields.into_iter().enumerate() {
        let Some(normalized) = value.text().and_then(|text| field.normalize(text)) else {
            submitted.set(true);
            focus_element_by_id_after_render(field.input_id());
            return None;
        };
        values[index] = normalized;
    }
    submitted.set(false);
    Some(values)
}

fn use_filter_draft<T: Clone + PartialEq + 'static>(
    applied: Memo<T>,
    mut submitted: Signal<bool>,
) -> Signal<T> {
    let mut draft = use_signal(move || applied.peek().clone());
    // 只有已应用条件改变才替换输入；同条件翻页或轮询不改变这个 memo。
    use_effect(move || {
        let next = applied();
        if *draft.peek() != next {
            draft.set(next);
        }
        submitted.set(false);
    });
    draft
}
