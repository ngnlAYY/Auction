//! 非敏感配置候选的逐项变更清单，按 Worker 热字段与重启字段展示实际差异。

use std::borrow::Cow;

use auction_web_shared::t;
use dioxus::prelude::*;
use serde_json::Value;

use super::early_bird;
use super::hot_fields::is_hot;

#[derive(Clone, PartialEq)]
pub(super) struct ConfigChange {
    path: String,
    before: Value,
    after: Value,
}

impl ConfigChange {
    pub(super) fn needs_restart(&self) -> bool {
        !is_hot(&self.path)
    }
}

pub(super) fn changes(before: &Value, after: &Value) -> Vec<ConfigChange> {
    let mut changes = Vec::new();
    collect_changes("", before, after, &mut changes);
    changes
}

fn collect_changes(path: &str, before: &Value, after: &Value, changes: &mut Vec<ConfigChange>) {
    let before = editable_value(path, before);
    let after = editable_value(path, after);
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
            path: path.to_owned(),
            before: before.into_owned(),
            after: after.into_owned(),
        });
    }
}

fn editable_value<'a>(path: &str, value: &'a Value) -> Cow<'a, Value> {
    // 物理文档省略空的 bot_players，表单则把它展示为空列表。
    if path == "bot_players" && value.as_array().is_some_and(Vec::is_empty) {
        return Cow::Borrowed(&Value::Null);
    }
    // 第一次编辑早鸟字段会补齐旧文档缺省的区块。评审以表单展示的默认值作为比较基线。
    if path == early_bird::ROOT && (value.is_null() || value.is_object()) {
        let mut block = early_bird::default_block();
        if let (Some(fields), Some(defaults)) = (value.as_object(), block.as_object_mut()) {
            defaults.extend(fields.clone());
        }
        return Cow::Owned(block);
    }
    Cow::Borrowed(value)
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
pub(super) fn ConfigChangeList(changes: Vec<ConfigChange>, pending_restart: bool) -> Element {
    let (restart, hot): (Vec<_>, Vec<_>) =
        changes.into_iter().partition(ConfigChange::needs_restart);
    rsx! {
        p { class: "admin-panel__note", {t!("admin-runtime-review-impact")} }
        if !hot.is_empty() {
            h3 { {t!("admin-runtime-review-hot")} }
            p { class: "admin-panel__note", {t!("admin-runtime-hot-field-title")} }
            if pending_restart {
                p { class: "admin-panel__note", {t!("admin-runtime-review-pending-restart")} }
            }
            ChangeRows { changes: hot }
        }
        if !restart.is_empty() {
            h3 { {t!("admin-runtime-review-restart")} }
            if restart.iter().any(|change| change.path.starts_with("equipment_colors")) {
                p { class: "admin-panel__note", {t!("admin-runtime-review-colors-impact")} }
            }
            if restart.iter().any(|change| change.path.starts_with("operations.economics.")) {
                p { class: "admin-panel__note", {t!("admin-runtime-review-economics-impact")} }
            }
            ChangeRows { changes: restart }
        }
    }
}

#[component]
fn ChangeRows(changes: Vec<ConfigChange>) -> Element {
    rsx! {
        dl { class: "admin-config-changes",
            for change in changes {
                div { key: "{change.path}", class: "admin-config-changes__entry",
                    dt { {field_label(&change.path)} }
                    dd {
                        span { class: "admin-config-changes__label", {t!("admin-runtime-review-before")} }
                        pre { {display_value(&change.before)} }
                    }
                    dd {
                        span { class: "admin-config-changes__label", {t!("admin-runtime-review-after")} }
                        pre { {display_value(&change.after)} }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
    use dioxus::{core::Mutation, prelude::*};
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
    use serde_json::json;

    use super::{ConfigChange, ConfigChangeList, changes, early_bird};
    use crate::surface::ADMIN_LOCALE_CATALOG;

    #[component]
    fn ReviewHarness(changes: Vec<ConfigChange>, pending_restart: bool) -> Element {
        try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_ADMIN_LOCALE,
                ADMIN_LOCALE_CATALOG.default_ftl,
                &DEFAULT_ADMIN_LOCALE,
                None,
            )
        })
        .unwrap();
        rsx! { ConfigChangeList { changes, pending_restart } }
    }

    #[test]
    fn review_equates_absent_and_empty_bot_players_in_both_directions() {
        let before = json!({"metrics": {"enabled": false}});
        let after = json!({"metrics": {"enabled": true}, "bot_players": []});
        for (source, candidate) in [(&before, &after), (&after, &before)] {
            let changed = changes(source, candidate);
            assert_eq!(changed.len(), 1);
            assert_eq!(changed[0].path, "metrics.enabled");
        }
    }

    #[test]
    fn review_counts_removing_configured_bot_players() {
        let changed =
            changes(&json!({"bot_players": ["Auction Bot"]}), &json!({"bot_players": []}));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, "bot_players");
    }

    #[test]
    fn review_counts_only_edited_fields_when_materializing_early_bird_defaults() {
        let before = json!({"operations": {"schedule": {"managed": true}}});
        let mut after = before.clone();
        after["operations"]["schedule"]["early_bird"] = early_bird::default_block();
        assert!(changes(&before, &after).is_empty());
        after["operations"]["schedule"]["early_bird"]["enabled"] = json!(true);
        let changed = changes(&before, &after);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].path, "operations.schedule.early_bird.enabled");
    }

    #[test]
    fn review_separates_actual_hot_and_restart_changes() {
        let before = json!({
            "operations": {"mail": {"managed": true, "send_classes": {
                "account_reply": true, "intake_reply": true,
            }}},
            "metrics": {"enabled": false},
        });
        let mut after = before.clone();
        after["operations"]["mail"]["send_classes"]["account_reply"] = json!(false);
        let hot_only = changes(&before, &after);
        assert_eq!(hot_only.len(), 1);
        assert!(!hot_only[0].needs_restart());

        after["metrics"]["enabled"] = json!(true);
        let mixed = changes(&before, &after);
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[0].path, "metrics.enabled");
        assert!(mixed[0].needs_restart());
        assert_eq!(mixed[1].path, "operations.mail.send_classes.account_reply");
        assert!(!mixed[1].needs_restart());
        assert!(changes(&before, &before).is_empty());
    }

    #[test]
    fn hot_review_includes_compatibility_and_calls_out_an_outstanding_restart() {
        let before = json!({"operations": {"forum": {
            "managed": true, "lease_duration_seconds": 600, "item_timeout_seconds": 240,
        }}});
        let mut after = before.clone();
        after["operations"]["forum"]["item_timeout_seconds"] = json!(360);
        for pending_restart in [false, true] {
            let mut dom = VirtualDom::new_with_props(
                ReviewHarness,
                ReviewHarnessProps { changes: changes(&before, &after), pending_restart },
            );
            let mutations = dom.rebuild_to_vec();
            let text_contains = |expected: &str| {
                mutations.edits.iter().any(|mutation| {
                    matches!(mutation, Mutation::CreateTextNode { value, .. } if value.contains(expected))
                })
            };
            assert!(text_contains("保存后由 Worker 热读"));
            assert!(text_contains("冲突的组沿用旧值，重启后采用新值"));
            assert_eq!(text_contains("此前保存的配置仍待重启"), pending_restart);
        }
    }
}
