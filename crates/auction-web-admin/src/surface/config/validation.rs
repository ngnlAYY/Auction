//! 配置表单的本地输入反馈；字段约束来自已渲染控件，服务端仍负责完整候选校验。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::model::RuntimeConfigForm;

#[derive(Clone, PartialEq)]
pub(super) struct FieldIssue {
    pub(super) path: String,
    pub(super) label: String,
    pub(super) message: String,
}

#[derive(Clone, Copy)]
pub(super) struct FormIssues(pub(super) Signal<Vec<FieldIssue>>);

pub(super) fn field_id(path: &str) -> String {
    format!("runtime-field-{}", path.replace('.', "-"))
}

/// 复用控件的 min/max 与标签，避免在错误摘要内维护第二份配置范围和译文映射。
pub(super) fn collect_issues(_form: &RuntimeConfigForm) -> Vec<FieldIssue> {
    let mut issues = Vec::new();
    #[cfg(target_arch = "wasm32")]
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Ok(fields) = document.query_selector_all("[data-runtime-number]")
    {
        use wasm_bindgen::JsCast;
        for index in 0..fields.length() {
            let Some(field) =
                fields.item(index).and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            if field.has_attribute("disabled") {
                continue;
            }
            let Some(path) = field.get_attribute("data-runtime-number") else { continue };
            if !_form.edits_field(&path) {
                continue;
            }
            let minimum = field.get_attribute("min").unwrap_or_default();
            let maximum = field.get_attribute("max").unwrap_or_default();
            let input = _form.input(&path);
            let message = match input.parse::<i128>() {
                Err(_) => Some(t!("admin-runtime-field-integer")),
                Ok(value)
                    if minimum.parse::<i128>().is_ok_and(|min| value < min)
                        || maximum.parse::<i128>().is_ok_and(|max| value > max) =>
                {
                    Some(t!("admin-runtime-field-range", minimum: minimum, maximum: maximum))
                }
                _ => None,
            };
            if let Some(message) = message {
                issues.push(FieldIssue { label: control_label(&field, &path), path, message });
            }
        }
    }
    // 原生目标只参与静态编译；浏览器控件的范围校验不会替代后续服务端规范化校验。
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &mut issues;
    issues
}

/// 将所属分组带入字段名称，避免多个“启用”或“超时”在摘要和变更清单里无法区分。
#[cfg(target_arch = "wasm32")]
pub(super) fn control_label(field: &web_sys::Element, path: &str) -> String {
    let mut label = field.get_attribute("data-runtime-label").unwrap_or_else(|| path.to_owned());
    let mut ancestor = field.parent_element();
    while let Some(element) = ancestor {
        if element.matches(".admin-runtime-group, .admin-runtime-section").unwrap_or(false)
            && let Some(title) = element
                .query_selector(":scope > h3, :scope > summary")
                .ok()
                .flatten()
                .and_then(|title| title.text_content())
        {
            label =
                t!("admin-runtime-field-context", context: title.trim().to_owned(), field: label);
        }
        ancestor = element.parent_element();
    }
    label
}

pub(super) fn focus_field(_path: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let Some(document) = web_sys::window().and_then(|window| window.document()) else { return };
        let Some(field) = document.get_element_by_id(&field_id(_path)) else { return };
        let mut ancestor = field.parent_element();
        while let Some(element) = ancestor {
            if element.tag_name() == "DETAILS" {
                let _ = element.set_attribute("open", "");
            }
            ancestor = element.parent_element();
        }
        if let Some(field) = field.dyn_ref::<web_sys::HtmlElement>() {
            let _ = field.focus();
            field.scroll_into_view_with_bool(true);
        }
    }
}

pub(super) fn focus_summary() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id("runtime-errors"))
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    }
}

#[component]
pub(super) fn ErrorSummary(issues: Vec<FieldIssue>) -> Element {
    rsx! {
        section {
            id: "runtime-errors", class: "admin-config-errors", tabindex: "-1",
            role: "alert", aria_labelledby: "runtime-errors-title",
            onmounted: move |_| focus_summary(),
            h3 { id: "runtime-errors-title", {t!("admin-runtime-field-errors")} }
            ul {
                for issue in issues {
                    li { key: "{issue.path}",
                        a {
                            href: "#{field_id(&issue.path)}",
                            onclick: move |event| { event.prevent_default(); focus_field(&issue.path); },
                            "{issue.label}: {issue.message}"
                        }
                    }
                }
            }
        }
    }
}
