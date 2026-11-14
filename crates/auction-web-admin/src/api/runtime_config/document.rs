//! 运行配置文档的业务段校验：世界、装备颜色与筛选、模板、装备来源与看板邮箱。

use auction_web_shared::wire::json_exact_object;
use serde_json::Value;

use super::fields::{
    bool_field, optional_keys_object, string_field, u64_field, valid_string_list, valid_text,
};
use super::infrastructure::{
    valid_listener, valid_metrics, valid_security, valid_session, valid_tracing,
    valid_worker_control,
};
use super::{operations, taxonomy};

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
        && document.get("bot_players").is_none_or(|value| valid_string_list(value, 16, 64, true))
        && document.get("equipment_colors").is_none_or(valid_equipment_colors)
        && valid_worlds(&document["worlds"])
        && valid_equipment_filter(&document["equipment_filter"])
        && document.get("mail_templates").is_none_or(valid_legacy_mail_templates)
        && valid_equipment_source(&document["equipment_source"])
        && valid_listener(&document["listener"])
        && valid_dashboard_mailboxes(&document["dashboard_mailboxes"])
        && valid_metrics(&document["metrics"])
        && valid_security(&document["security"])
        && valid_session(&document["session"])
        && valid_tracing(&document["tracing"])
        && valid_worker_control(&document["worker_control"])
        && operations::valid_operations(&document["operations"])
}

pub(super) fn valid_worlds(value: &Value) -> bool {
    let Some(worlds) = json_exact_object(value, &["isekai", "main"]) else {
        return false;
    };
    let Some(main_enabled) = valid_runtime_world(&worlds["main"]) else {
        return false;
    };
    let Some(isekai_enabled) = valid_runtime_world(&worlds["isekai"]) else {
        return false;
    };
    main_enabled || isekai_enabled
}

pub(super) fn valid_runtime_world(value: &Value) -> Option<bool> {
    let world =
        json_exact_object(value, &["enabled", "sender_allowlist", "sender_allowlist_disabled"])?;
    let enabled = bool_field(world, "enabled")?;
    bool_field(world, "sender_allowlist_disabled")?;
    valid_string_list(&world["sender_allowlist"], 1000, 128, true).then_some(enabled)
}

pub(super) fn valid_equipment_colors(value: &Value) -> bool {
    let Some(colors) = json_exact_object(value, &["rules"]) else {
        return false;
    };
    let Some(rules) = colors["rules"].as_array() else {
        return false;
    };
    rules.len() <= 200 && rules.iter().all(valid_equipment_color_rule)
}

pub(super) fn valid_equipment_color_rule(value: &Value) -> bool {
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
    if [
        "category",
        "combined_color",
        "equipment_type",
        "level",
        "prefix_color",
        "prefixes",
        "quality",
        "suffix_color",
        "suffixes",
    ]
    .iter()
    .any(|key| !rule.contains_key(*key))
    {
        return false;
    }
    let Some(category) = string_field(rule, "category") else {
        return false;
    };
    let Some(equipment_type) = string_field(rule, "equipment_type") else {
        return false;
    };
    let Some(quality) = string_field(rule, "quality") else {
        return false;
    };
    let Some(level) = json_exact_object(&rule["level"], &["max", "min"]) else {
        return false;
    };
    let Some(minimum) = level["min"].as_i64() else {
        return false;
    };
    let Some(maximum) = level["max"].as_i64() else {
        return false;
    };
    let prefixes_valid = valid_string_list(&rule["prefixes"], 256, 128, true);
    let suffixes_valid = valid_string_list(&rule["suffixes"], 256, 128, true);
    let has_prefix_or_suffix = rule["prefixes"].as_array().is_some_and(|values| !values.is_empty())
        || rule["suffixes"].as_array().is_some_and(|values| !values.is_empty());
    valid_color_selection(category, rule.get("categories"))
        && valid_color_selection(equipment_type, rule.get("equipment_types"))
        && valid_color_selection(quality, rule.get("qualities"))
        && (0..=1000).contains(&minimum)
        && (0..=1000).contains(&maximum)
        && (maximum == 0 || maximum >= minimum)
        && prefixes_valid
        && suffixes_valid
        && has_prefix_or_suffix
        && ["prefix_color", "suffix_color", "combined_color"]
            .iter()
            .all(|key| rule[*key].as_str().is_some_and(valid_hex_color))
}

/// 多选维度沿用旧单值兼容，数组内按 ASCII 忽略大小写去重且不能同时声明非空单值。
pub(super) fn valid_color_selection(singular: &str, plural: Option<&Value>) -> bool {
    if !valid_text(singular, 64, true) {
        return false;
    }
    let Some(plural) = plural else { return true };
    let Some(values) = plural.as_array() else { return false };
    values.len() <= 256
        && (values.is_empty() || singular.is_empty())
        && values.iter().all(Value::is_string)
        && taxonomy::valid_names(values.iter().filter_map(Value::as_str), 256, 64, true)
}

pub(super) fn valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

pub(super) fn valid_equipment_filter(value: &Value) -> bool {
    let Some(filter) = json_exact_object(
        value,
        &["categories", "enabled", "equipment_types", "level", "prefixes", "qualities", "suffixes"],
    ) else {
        return false;
    };
    let Some(enabled) = bool_field(filter, "enabled") else {
        return false;
    };
    let lists = ["qualities", "prefixes", "suffixes", "categories", "equipment_types"];
    if lists.iter().any(|name| !valid_string_list(&filter[*name], 256, 128, true)) {
        return false;
    }
    let Some(level) = json_exact_object(&filter["level"], &["max", "min"]) else {
        return false;
    };
    let Some(minimum) = level["min"].as_i64() else {
        return false;
    };
    let Some(maximum) = level["max"].as_i64() else {
        return false;
    };
    if !(0..=1000).contains(&minimum)
        || !(0..=1000).contains(&maximum)
        || maximum != 0 && maximum < minimum
    {
        return false;
    }
    !enabled
        || minimum != 0
        || maximum != 0
        || lists
            .iter()
            .any(|name| filter[*name].as_array().is_some_and(|values| !values.is_empty()))
}

/// runtime 文档里的历史模板区块：通知模板已迁入数据库，这里的正文不再生效，页面不展示也不
/// 编辑，只随文档原样往返（旧程序加载 runtime 文件时仍要求它存在）。因此只核对形状，不按
/// 占位符规则校验内容。
pub(super) fn valid_legacy_mail_templates(value: &Value) -> bool {
    value.as_object().is_some_and(|templates| {
        templates.len() <= 64 && templates.values().all(|body| body.as_str().is_some())
    })
}

pub(super) fn valid_equipment_source(value: &Value) -> bool {
    let Some(source) = json_exact_object(
        value,
        &[
            "allowed_hosts",
            "allowed_path_prefixes",
            "allowlist_enabled",
            "maximum_response_bytes",
            "minimum_request_interval_millis",
            "require_tradeable",
            "request_timeout_seconds",
        ],
    ) else {
        return false;
    };
    let Some(enabled) = bool_field(source, "allowlist_enabled") else {
        return false;
    };
    if bool_field(source, "require_tradeable").is_none() {
        return false;
    }
    let hosts_valid = valid_string_list(&source["allowed_hosts"], 16, 253, true);
    let paths_valid = valid_string_list(&source["allowed_path_prefixes"], 16, 512, true);
    let hosts_present = source["allowed_hosts"].as_array().is_some_and(|values| !values.is_empty());
    let paths_present =
        source["allowed_path_prefixes"].as_array().is_some_and(|values| !values.is_empty());
    hosts_valid
        && paths_valid
        && (!enabled || hosts_present && paths_present)
        && u64_field(source, "maximum_response_bytes")
            .is_some_and(|value| (64 << 10..=8 << 20).contains(&value))
        && u64_field(source, "minimum_request_interval_millis")
            .is_some_and(|value| (1..=60_000).contains(&value))
        && u64_field(source, "request_timeout_seconds")
            .is_some_and(|value| (1..=30).contains(&value))
}

pub(super) fn valid_dashboard_mailboxes(value: &Value) -> bool {
    let Some(mailboxes) = value.as_array() else {
        return false;
    };
    if mailboxes.len() > 2 {
        return false;
    }
    let mut worlds = Vec::new();
    let mut keys = Vec::new();
    mailboxes.iter().all(|value| {
        let Some(mailbox) = json_exact_object(value, &["mailbox_key", "world_id"]) else {
            return false;
        };
        let Some(world) = string_field(mailbox, "world_id") else {
            return false;
        };
        let Some(key) = string_field(mailbox, "mailbox_key") else {
            return false;
        };
        matches!(world, "main" | "isekai")
            && valid_text(key, 256, false)
            && !worlds.contains(&world)
            && !keys.contains(&key)
            && {
                worlds.push(world);
                keys.push(key);
                true
            }
    })
}
