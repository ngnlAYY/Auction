//! 分页数量预设与正整数输入；只提交可表示的合法整数，不截断或静默修正用户输入。

use dioxus::prelude::*;

/// 数量文案由 surface 注入；显式启用才显示自定义输入，不设业务数量上限。
#[component]
pub(super) fn PageSizePicker(
    page_size: i64,
    options: Vec<i64>,
    label: String,
    option_label: Option<Callback<i64, String>>,
    allow_custom: bool,
    custom_label: String,
    apply_label: String,
    invalid_label: String,
    on_change: EventHandler<i64>,
) -> Element {
    let mut input = use_signal(|| page_size.to_string());
    let mut touched = use_signal(|| false);
    let hint_id = use_hook(|| format!("page-size-hint-{}", uuid::Uuid::new_v4()));
    use_effect(use_reactive((&page_size,), move |(size,)| {
        input.set(size.to_string());
        touched.set(false);
    }));

    // 不用浮点转换或 type=number：科学计数法、符号、空白和小数都不能成为数量。
    let value = input();
    let target = (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<i64>().ok())
        .flatten()
        .filter(|size| *size > 0);
    let invalid = touched() && target.is_none();
    let custom_selected = !options.contains(&page_size);
    let allowed_sizes = options.clone();

    rsx! {
        label { class: "ui-pager__size",
            span { class: "ui-pager__sr-only", "{label}" }
            select {
                class: "ui-pager__select",
                name: "page_size",
                value: page_size.to_string(),
                onchange: move |event| {
                    if let Ok(size) = event.value().parse::<i64>()
                        && allowed_sizes.contains(&size)
                    {
                        on_change.call(size);
                    }
                },
                for size in options {
                    option {
                        key: "{size}",
                        value: size.to_string(),
                        selected: size == page_size,
                        {option_label.map_or_else(|| size.to_string(), |label| label.call(size))}
                    }
                }
                if custom_selected {
                    option { value: page_size.to_string(), selected: true,
                        {option_label.map_or_else(|| page_size.to_string(), |label| label.call(page_size))}
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
                        on_change.call(size);
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
                        aria_describedby: hint_id,
                        aria_invalid: if invalid { "true" } else { "false" },
                        title: invalid_label.clone(),
                        value,
                        oninput: move |event| input.set(event.value()),
                        onblur: move |_| touched.set(true),
                    }
                }
                button { r#type: "submit", disabled: target.is_none(), "{apply_label}" }
                if invalid {
                    span { class: "ui-pager__error", role: "status", "{invalid_label}" }
                }
            }
        }
    }
}
