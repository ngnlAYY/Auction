//! 配置表单的本地输入反馈；字段约束来自已渲染控件，服务端仍负责完整候选校验。

#[cfg(any(target_arch = "wasm32", test))]
use std::collections::BTreeMap;
#[cfg(all(test, not(target_arch = "wasm32")))]
use std::{cell::RefCell, rc::Rc};

use auction_web_shared::a11y::focus_element_by_id;
use auction_web_shared::t;
use dioxus::prelude::*;

use super::model::{MAX_DASHBOARD_MAILBOXES, RuntimeConfigForm};

pub(super) const COLOR_LEVEL_MINIMUM: &str = "0";
pub(super) const COLOR_LEVEL_MAXIMUM: &str = "1000";

#[derive(Clone, PartialEq)]
pub(super) struct FieldIssue {
    pub(super) path: String,
    kind: FieldIssueKind,
}

#[derive(Clone, PartialEq)]
enum FieldIssueKind {
    MailboxCount,
    ColorLevel {
        number: usize,
        is_minimum: bool,
        issue: NumberIssue,
    },
    #[cfg(any(target_arch = "wasm32", test))]
    Number {
        label_at_validation: String,
        minimum: String,
        maximum: String,
        issue: NumberIssue,
    },
}

impl FieldIssue {
    fn text(&self, _control_label: Option<&str>) -> String {
        let (label, message) = match &self.kind {
            FieldIssueKind::MailboxCount => {
                (t!("admin-runtime-section-mailboxes"), mailbox_count_error())
            }
            FieldIssueKind::ColorLevel { number, is_minimum, issue } => {
                let field = if *is_minimum {
                    t!("admin-runtime-color-level-min")
                } else {
                    t!("admin-runtime-color-level-max")
                };
                let context = t!("admin-runtime-color-rule", number: *number);
                (
                    t!("admin-runtime-field-context", context: context, field: field),
                    issue.message(COLOR_LEVEL_MINIMUM, COLOR_LEVEL_MAXIMUM),
                )
            }
            #[cfg(any(target_arch = "wasm32", test))]
            FieldIssueKind::Number { label_at_validation, minimum, maximum, issue } => (
                _control_label.unwrap_or(label_at_validation).to_owned(),
                issue.message(minimum, maximum),
            ),
        };
        format!("{label}: {message}")
    }
}

#[derive(Clone, Copy)]
pub(super) struct FormIssues(pub(super) Signal<Vec<FieldIssue>>);

pub(super) fn field_id(path: &str) -> String {
    format!("runtime-field-{}", path.replace('.', "-"))
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum NumberIssue {
    Integer,
    Range,
}

impl NumberIssue {
    pub(super) fn message(self, minimum: &str, maximum: &str) -> String {
        match self {
            Self::Integer => t!("admin-runtime-field-integer"),
            Self::Range => t!("admin-runtime-field-range", minimum: minimum, maximum: maximum),
        }
    }
}

pub(super) fn number_issue(input: &str, minimum: &str, maximum: &str) -> Option<NumberIssue> {
    let Ok(value) = input.parse::<i128>() else {
        return Some(NumberIssue::Integer);
    };
    if minimum.parse::<i128>().is_ok_and(|min| value < min)
        || maximum.parse::<i128>().is_ok_and(|max| value > max)
    {
        return Some(NumberIssue::Range);
    }
    None
}

pub(super) fn mailbox_count_error() -> String {
    t!("admin-runtime-mailbox-too-many", maximum: MAX_DASHBOARD_MAILBOXES)
}

pub(super) fn color_level_path(row_id: usize, bound: &str) -> String {
    format!("equipment_colors.rules.{row_id}.level.{bound}")
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(super) struct NumberControl {
    pub(super) path: String,
    pub(super) minimum: String,
    pub(super) maximum: String,
    pub(super) label: String,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Clone, Default)]
pub(super) struct NumberDocument(pub(super) Rc<RefCell<Vec<NumberControl>>>);

/// 历史值可以就地标错。摘要只拦截编辑后的输入，完整候选仍由后端校验。
pub(super) fn collect_issues(form: &RuntimeConfigForm) -> Vec<FieldIssue> {
    let mut issues = Vec::new();
    if form.edited_mailboxes_exceed_limit() {
        issues.push(FieldIssue {
            path: "dashboard_mailboxes".to_owned(),
            kind: FieldIssueKind::MailboxCount,
        });
    }
    for (index, rule) in form.equipment_colors().iter().enumerate() {
        for (bound, input) in [("min", &rule.level_min), ("max", &rule.level_max)] {
            if form
                .source_color_level(index, bound)
                .is_some_and(|source| source.to_string() == *input)
            {
                continue;
            }
            if let Some(issue) = number_issue(input, COLOR_LEVEL_MINIMUM, COLOR_LEVEL_MAXIMUM) {
                issues.push(FieldIssue {
                    path: color_level_path(rule.row_id, bound),
                    kind: FieldIssueKind::ColorLevel {
                        number: index + 1,
                        is_minimum: bound == "min",
                        issue,
                    },
                });
            }
        }
    }
    #[cfg(any(target_arch = "wasm32", test))]
    visit_number_controls(|path, minimum, maximum, label| {
        if let Some(input) = form.edited_input(path)
            && let Some(issue) = number_issue(input, minimum, maximum)
        {
            issues.push(FieldIssue {
                path: path.to_owned(),
                kind: FieldIssueKind::Number {
                    label_at_validation: label(),
                    minimum: minimum.to_owned(),
                    maximum: maximum.to_owned(),
                    issue,
                },
            });
        }
    });
    issues
}

/// 沿用已挂载控件的范围和标签；只在确认有错误后读取其完整分组名。
#[cfg(any(target_arch = "wasm32", test))]
fn visit_number_controls(mut visit: impl FnMut(&str, &str, &str, &dyn Fn() -> String)) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let Some(fields) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.query_selector_all("[data-runtime-number]").ok())
        else {
            return;
        };
        for index in 0..fields.length() {
            let Some(field) =
                fields.item(index).and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            if field.has_attribute("disabled") {
                continue;
            }
            if let Some(path) = field.get_attribute("data-runtime-number") {
                let minimum = field.get_attribute("min").unwrap_or_default();
                let maximum = field.get_attribute("max").unwrap_or_default();
                visit(&path, &minimum, &maximum, &|| control_label(&field, &path));
            }
        }
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(document) = try_consume_context::<NumberDocument>() {
        for field in document.0.borrow().iter() {
            visit(&field.path, &field.minimum, &field.maximum, &|| field.label.clone());
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn current_number_label(path: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_sys::window()?.document()?;
        let field = document.get_element_by_id(&field_id(path))?;
        Some(control_label(&field, path))
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    {
        let document = try_consume_context::<NumberDocument>()?;
        let controls = document.0.borrow();
        controls.iter().find(|control| control.path == path).map(|control| control.label.clone())
    }
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
    focus_element_by_id("runtime-errors");
}

#[component]
pub(super) fn ErrorSummary(issues: Vec<FieldIssue>) -> Element {
    #[cfg(any(target_arch = "wasm32", test))]
    let numbers = {
        let mut numbers = use_signal(BTreeMap::new);
        use_effect(use_reactive((&issues,), move |(issues,)| {
            // 控件与祖先标题先完成语言更新，再读取实际显示的层级名称与同语种的错误正文。
            let next = issues
                .iter()
                .filter_map(|issue| {
                    if !matches!(issue.kind, FieldIssueKind::Number { .. }) {
                        return None;
                    }
                    let label = current_number_label(&issue.path);
                    Some((issue.path.clone(), (issue.kind.clone(), issue.text(label.as_deref()))))
                })
                .collect();
            if *numbers.peek() != next {
                numbers.set(next);
            }
        }));
        numbers
    };
    #[cfg(any(target_arch = "wasm32", test))]
    let numbers = numbers.read();
    let rows = issues.into_iter().map(|issue| {
        #[cfg(any(target_arch = "wasm32", test))]
        if let Some((kind, text)) = numbers.get(&issue.path)
            && *kind == issue.kind
        {
            return (issue.path, text.clone());
        }
        let text = issue.text(None);
        (issue.path, text)
    });
    rsx! {
        section {
            id: "runtime-errors", class: "admin-config-errors", tabindex: "-1",
            role: "alert", aria_labelledby: "runtime-errors-title",
            onmounted: move |_| focus_summary(),
            h3 { id: "runtime-errors-title", {t!("admin-runtime-field-errors")} }
            ul {
                for (path, message) in rows {
                    li { key: "{path}",
                        a {
                            href: "#{field_id(&path)}",
                            onclick: move |event| { event.prevent_default(); focus_field(&path); },
                            "{message}"
                        }
                    }
                }
            }
        }
    }
}
