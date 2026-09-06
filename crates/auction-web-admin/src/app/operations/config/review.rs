//! 非敏感配置候选的逐项变更清单；只比较安全 JSON 投影，不推断生效状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use serde_json::Value;

#[derive(Clone, PartialEq)]
pub(super) struct ConfigChange {
    pub(super) path: String,
    pub(super) label: String,
    pub(super) before: String,
    pub(super) after: String,
}

pub(super) fn changes(before: &Value, after: &Value) -> Vec<ConfigChange> {
    let mut changes = Vec::new();
    collect_changes("", before, after, &mut changes);
    changes
}

fn collect_changes(path: &str, before: &Value, after: &Value, changes: &mut Vec<ConfigChange>) {
    if before == after {
        return;
    }
    if let (Some(previous), Some(candidate)) = (before.as_object(), after.as_object()) {
        let keys =
            previous.keys().chain(candidate.keys()).collect::<std::collections::BTreeSet<_>>();
        for key in keys {
            let field = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            collect_changes(
                &field,
                previous.get(key).unwrap_or(&Value::Null),
                candidate.get(key).unwrap_or(&Value::Null),
                changes,
            );
        }
    } else {
        changes.push(ConfigChange {
            label: field_label(path),
            path: path.to_owned(),
            before: display_value(before),
            after: display_value(after),
        });
    }
}

fn field_label(path: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    if let Some(label) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&super::validation::field_id(path)))
        .map(|field| super::validation::control_label(&field, path))
    {
        return label;
    }
    if path == "dashboard_mailboxes" {
        return t!("admin-runtime-section-mailboxes");
    }
    path.to_owned()
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Bool(true) => t!("admin-runtime-enabled"),
        Value::Bool(false) => t!("admin-runtime-disabled"),
        Value::String(text) if !text.is_empty() => text.clone(),
        Value::String(_) | Value::Null => t!("admin-runtime-review-empty"),
        _ => value.to_string(),
    }
}

#[component]
pub(super) fn ConfigChangeList(changes: Vec<ConfigChange>) -> Element {
    rsx! {
        p { class: "admin-panel__note", {t!("admin-runtime-review-impact")} }
        dl { class: "admin-config-changes",
            for change in changes {
                div { key: "{change.path}", class: "admin-config-changes__entry",
                    dt { "{change.label}" }
                    dd {
                        span { class: "admin-config-changes__label", {t!("admin-runtime-review-before")} }
                        pre { "{change.before}" }
                    }
                    dd {
                        span { class: "admin-config-changes__label", {t!("admin-runtime-review-after")} }
                        pre { "{change.after}" }
                    }
                }
            }
        }
    }
}
