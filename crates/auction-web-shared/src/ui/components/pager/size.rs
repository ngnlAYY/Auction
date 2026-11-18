//! 分页数量预设与正整数输入；只提交可表示的合法整数，不截断或静默修正用户输入。
//! 回调属性须为非可选类型，避免更新时拷入会随临时 VNode 释放的句柄
//! （Dioxus 0.7.10 的 `dioxus-core-macro/src/props/mod.rs:694–715` 与 `dioxus-core/src/events.rs:555–556`）。

use dioxus::dioxus_core::needs_update;
use dioxus::prelude::*;

use crate::validation::restore_input_value;

/// 数量文案与可选上限由 surface 注入；显式启用才显示自定义输入。
#[component]
pub(super) fn PageSizePicker(
    page_size: i64,
    options: Vec<i64>,
    label: String,
    #[props(default = |size: i64| size.to_string())] option_label: Callback<i64, String>,
    allow_custom: bool,
    max_page_size: Option<i64>,
    custom_label: String,
    apply_label: String,
    invalid_label: String,
    on_change: Callback<i64, bool>,
) -> Element {
    let mut input = use_signal(|| page_size.to_string());
    let mut touched = use_signal(|| false);
    let mut input_element = use_signal(|| None::<MountedEvent>);
    let hint_id = use_hook(|| format!("page-size-hint-{}", uuid::Uuid::new_v4()));
    use_effect(use_reactive((&page_size,), move |(size,)| {
        input.set(size.to_string());
        touched.set(false);
    }));

    // 不用浮点转换或 type=number：科学计数法、符号、空白和小数都不能成为数量。
    let value = input();
    let target = parse_size(&value, max_page_size);
    let invalid = touched() && target.is_none();
    let custom_selected = !options.contains(&page_size);
    let allowed_sizes = options.clone();

    rsx! {
        label { class: "ui-pager__size",
            span { class: "ui-pager__sr-only", "{label}" }
            select {
                class: "ui-pager__select",
                name: "page_size",
                autocomplete: "off",
                value: page_size.to_string(),
                onchange: move |event| {
                    let value = event.value();
                    let size = parse_size(&value, max_page_size)
                        .filter(|size| allowed_sizes.contains(size));
                    // 原生 change 已改变选项；拒写后重绘 volatile value，把已应用数量写回同一控件。
                    apply_size_selection(size, page_size, |size| on_change.call(size),
                        |_| needs_update());
                },
                for size in options {
                    option {
                        key: "{size}",
                        value: size.to_string(),
                        selected: size == page_size,
                        disabled: size <= 0 || max_page_size.is_some_and(|maximum| size > maximum),
                        {option_label.call(size)}
                    }
                }
                if custom_selected {
                    option { value: page_size.to_string(), selected: true,
                        {option_label.call(page_size)}
                    }
                }
            }
        }
        if allow_custom {
            form {
                class: "ui-pager__custom",
                novalidate: true,
                onsubmit: move |event| {
                    event.prevent_default();
                    touched.set(true);
                    if let Some(size) = target {
                        if !on_change.call(size) {
                            let applied = page_size.to_string();
                            if let Some(element) = input_element.peek().as_ref() {
                                restore_input_value(element, &applied);
                            }
                            input.set(applied);
                            touched.set(false);
                        }
                    } else if let Some(input) = input_element() {
                        spawn(async move {
                            let _ = input.set_focus(true).await;
                        });
                    }
                },
                span { id: hint_id.clone(), class: "ui-pager__sr-only", "{invalid_label}" }
                label { class: "ui-pager__label",
                    span { "{custom_label}" }
                    input {
                        class: "ui-pager__input",
                        r#type: "text",
                        name: "custom_page_size",
                        inputmode: "numeric",
                        pattern: "[0-9]*[1-9][0-9]*",
                        required: true,
                        autocomplete: "off",
                        enterkeyhint: "done",
                        onmounted: move |event| input_element.set(Some(event)),
                        aria_describedby: hint_id,
                        aria_invalid: if invalid { "true" } else { "false" },
                        title: invalid_label,
                        value,
                        oninput: move |event| input.set(event.value()),
                        onblur: move |_| touched.set(true),
                    }
                }
                button { r#type: "submit", "{apply_label}" }
                if invalid {
                    span { class: "ui-pager__error", role: "status", "{invalid_label}" }
                }
            }
        }
    }
}

fn parse_size(value: &str, maximum: Option<i64>) -> Option<i64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<i64>().ok())
        .flatten()
        .filter(|size| *size > 0 && maximum.is_none_or(|maximum| *size <= maximum))
}

fn apply_size_selection(
    size: Option<i64>,
    applied: i64,
    commit: impl FnOnce(i64) -> bool,
    restore: impl FnOnce(i64),
) {
    if size.is_some_and(|size| !commit(size)) {
        restore(applied);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{apply_size_selection, parse_size};

    #[test]
    fn native_selection_restores_applied_size_when_the_commit_is_rejected() {
        let value = Cell::new(50);
        for accepted in [false, true] {
            value.set(50);
            apply_size_selection(
                parse_size("50", Some(100)),
                20,
                |size| {
                    assert_eq!(size, 50);
                    accepted
                },
                |size| value.set(size),
            );
            assert_eq!(value.get(), if accepted { 50 } else { 20 });
        }
        assert_eq!(parse_size("201", Some(200)), None);
    }
}
