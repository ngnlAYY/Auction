//! 运行配置候选的保存回执投影，以及配置文档的业务段校验。

use auction_web_shared::wire::json_exact_object;
use serde_json::{Map, Value, json};

use super::fields::{
    bool_field, optional_keys_object, safe_i64, string_field, valid_scalars, valid_string_array,
};
use super::infrastructure::{
    valid_listener, valid_metrics, valid_security, valid_session, valid_tracing,
    valid_worker_control,
};
use super::operations;

/// 将完整的页面候选转成 HTTP 保存回执的文档投影，供结构相等比较。
pub(super) fn project_update_document(value: &mut Value) {
    let Some(document) = value.as_object_mut() else { return };
    // HTTP 已补齐邮件默认值、可交易开关和告警阈值，候选中含这些字段时会按原值回显。
    omit_empty_arrays(document, &["bot_players"]);
    let colors = document.entry("equipment_colors").or_insert_with(|| json!({ "rules": [] }));
    if let Some(rules) = colors.get_mut("rules").and_then(Value::as_array_mut) {
        for rule in rules.iter_mut().filter_map(Value::as_object_mut) {
            omit_empty_arrays(rule, &["categories", "equipment_types", "qualities"]);
        }
    }
    if let Some(qualities) =
        value.pointer_mut("/equipment_filter/qualities").and_then(Value::as_array_mut)
        && qualities.is_empty()
    {
        // HTTP 以实际生效的品质门槛投影空配置（materialize_business_defaults）。
        qualities.extend([Value::from("Legendary"), Value::from("Peerless")]);
    }
    if let Some(early_bird) =
        value.pointer_mut("/operations/schedule/early_bird").and_then(Value::as_object_mut)
    {
        if early_bird.get("pre_season_session").and_then(Value::as_bool) == Some(true) {
            early_bird.remove("pre_season_session");
        }
        if early_bird.get("pre_season_days_before").and_then(Value::as_u64) == Some(3) {
            early_bird.remove("pre_season_days_before");
        }
    }
}

fn omit_empty_arrays(object: &mut Map<String, Value>, keys: &[&str]) {
    for &key in keys {
        if object.get(key).and_then(Value::as_array).is_some_and(Vec::is_empty) {
            object.remove(key);
        }
    }
}

pub(super) fn valid_runtime_document(value: &Value) -> bool {
    let Some(document) = optional_keys_object(
        value,
        &[
            "allow_manual_early_open",
            "allow_manual_early_settlement",
            "bot_players",
            "dashboard_mailboxes",
            "equipment_colors",
            "equipment_filter",
            "equipment_source",
            "listener",
            "mail_templates",
            "metrics",
            "operations",
            "security",
            "session",
            "tracing",
            "worker_control",
            "worlds",
        ],
    ) else {
        return false;
    };
    bool_field(document, "allow_manual_early_open").is_some()
        && bool_field(document, "allow_manual_early_settlement").is_some()
        && document.get("bot_players").is_none_or(valid_string_array)
        && document.get("equipment_colors").is_none_or(valid_equipment_colors)
        && document.get("worlds").is_some_and(valid_worlds)
        && document.get("equipment_filter").is_some_and(valid_equipment_filter)
        && document.get("mail_templates").is_none_or(valid_legacy_mail_templates)
        && document.get("equipment_source").is_some_and(valid_equipment_source)
        && document.get("listener").is_some_and(valid_listener)
        && document.get("dashboard_mailboxes").is_some_and(valid_dashboard_mailboxes)
        && document.get("metrics").is_some_and(valid_metrics)
        && document.get("security").is_some_and(valid_security)
        && document.get("session").is_some_and(valid_session)
        && document.get("tracing").is_some_and(valid_tracing)
        && document.get("worker_control").is_some_and(valid_worker_control)
        && document.get("operations").is_some_and(operations::valid_operations)
}

fn valid_worlds(value: &Value) -> bool {
    json_exact_object(value, &["isekai", "main"])
        .is_some_and(|worlds| worlds.values().all(valid_runtime_world))
}

fn valid_runtime_world(value: &Value) -> bool {
    valid_scalars(value, &["enabled", "sender_allowlist_disabled"], &[], &[], &["sender_allowlist"])
        && valid_string_array(&value["sender_allowlist"])
}

fn valid_equipment_colors(value: &Value) -> bool {
    let Some(colors) = json_exact_object(value, &["rules"]) else {
        return false;
    };
    let Some(rules) = colors.get("rules").and_then(Value::as_array) else {
        return false;
    };
    // 每条规则展开完整装备词典的复选组，限制编辑器一次构建的控件数量。
    rules.len() <= 200 && rules.iter().all(valid_equipment_color_rule)
}

fn valid_equipment_color_rule(value: &Value) -> bool {
    let Some(rule) = optional_keys_object(
        value,
        &[
            "categories",
            "category",
            "combined_color",
            "equipment_type",
            "equipment_types",
            "level",
            "prefix_color",
            "prefixes",
            "quality",
            "qualities",
            "suffix_color",
            "suffixes",
        ],
    ) else {
        return false;
    };
    ["category", "equipment_type", "quality", "prefix_color", "suffix_color", "combined_color"]
        .iter()
        .all(|key| string_field(rule, key).is_some())
        && ["categories", "equipment_types", "qualities"]
            .iter()
            .all(|key| rule.get(*key).is_none_or(valid_string_array))
        && rule.get("level").is_some_and(valid_level)
        && ["prefixes", "suffixes"].iter().all(|key| rule.get(*key).is_some_and(valid_string_array))
}

fn valid_level(value: &Value) -> bool {
    json_exact_object(value, &["max", "min"])
        .is_some_and(|level| level.values().all(|value| safe_i64(value).is_some()))
}

fn valid_equipment_filter(value: &Value) -> bool {
    let lists = ["qualities", "prefixes", "suffixes", "categories", "equipment_types"];
    valid_scalars(
        value,
        &["enabled"],
        &[],
        &[],
        &["categories", "equipment_types", "level", "prefixes", "qualities", "suffixes"],
    ) && valid_level(&value["level"])
        && lists.iter().all(|key| valid_string_array(&value[*key]))
}

/// 已迁入数据库的历史模板原样往返，浏览器只核对对象形状。
fn valid_legacy_mail_templates(value: &Value) -> bool {
    value.as_object().is_some_and(|templates| templates.values().all(Value::is_string))
}

fn valid_equipment_source(value: &Value) -> bool {
    valid_scalars(
        value,
        &["allowlist_enabled", "require_tradeable"],
        &[],
        &["maximum_response_bytes", "minimum_request_interval_millis", "request_timeout_seconds"],
        &["allowed_hosts", "allowed_path_prefixes"],
    ) && valid_string_array(&value["allowed_hosts"])
        && valid_string_array(&value["allowed_path_prefixes"])
}

fn valid_dashboard_mailboxes(value: &Value) -> bool {
    value.as_array().is_some_and(|mailboxes| {
        mailboxes.iter().all(|mailbox| {
            valid_scalars(mailbox, &[], &["mailbox_key", "world_id"], &[], &[])
                && matches!(mailbox["world_id"].as_str(), Some("main" | "isekai"))
        })
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::default_response;
    use super::valid_runtime_document;

    #[test]
    fn runtime_document_returns_failure_when_required_keys_are_missing() {
        let mut projection = default_response().document;
        projection["mail_templates"] = json!({"register_result": "旧正文"});
        for key in [
            "allow_manual_early_open",
            "allow_manual_early_settlement",
            "worlds",
            "equipment_filter",
            "equipment_source",
            "listener",
            "dashboard_mailboxes",
            "metrics",
            "security",
            "session",
            "tracing",
            "worker_control",
            "operations",
        ] {
            let mut missing = projection.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(!valid_runtime_document(&missing), "{key}");
        }
        let object = projection.as_object_mut().unwrap();
        object.remove("worlds");
        object.remove("equipment_filter");
        assert!(!valid_runtime_document(&projection));
    }
}
