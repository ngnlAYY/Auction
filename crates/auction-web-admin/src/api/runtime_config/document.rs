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

pub(super) fn valid_worlds(value: &Value) -> bool {
    let Some(worlds) = json_exact_object(value, &["isekai", "main"]) else {
        return false;
    };
    let Some(main_enabled) = worlds.get("main").and_then(valid_runtime_world) else {
        return false;
    };
    let Some(isekai_enabled) = worlds.get("isekai").and_then(valid_runtime_world) else {
        return false;
    };
    main_enabled || isekai_enabled
}

pub(super) fn valid_runtime_world(value: &Value) -> Option<bool> {
    let world =
        json_exact_object(value, &["enabled", "sender_allowlist", "sender_allowlist_disabled"])?;
    let enabled = bool_field(world, "enabled")?;
    bool_field(world, "sender_allowlist_disabled")?;
    valid_string_list(world.get("sender_allowlist")?, 1000, 128, true).then_some(enabled)
}

pub(super) fn valid_equipment_colors(value: &Value) -> bool {
    let Some(colors) = json_exact_object(value, &["rules"]) else {
        return false;
    };
    let Some(rules) = colors.get("rules").and_then(Value::as_array) else {
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
    let Some(category) = string_field(rule, "category") else {
        return false;
    };
    let Some(equipment_type) = string_field(rule, "equipment_type") else {
        return false;
    };
    let Some(quality) = string_field(rule, "quality") else {
        return false;
    };
    let Some(level) = rule.get("level").and_then(|level| json_exact_object(level, &["max", "min"]))
    else {
        return false;
    };
    let Some(minimum) = level.get("min").and_then(Value::as_i64) else {
        return false;
    };
    let Some(maximum) = level.get("max").and_then(Value::as_i64) else {
        return false;
    };
    let (Some(prefixes), Some(suffixes)) = (rule.get("prefixes"), rule.get("suffixes")) else {
        return false;
    };
    let has_prefix_or_suffix = prefixes.as_array().is_some_and(|values| !values.is_empty())
        || suffixes.as_array().is_some_and(|values| !values.is_empty());
    valid_color_selection(category, rule.get("categories"))
        && valid_color_selection(equipment_type, rule.get("equipment_types"))
        && valid_color_selection(quality, rule.get("qualities"))
        && (0..=1000).contains(&minimum)
        && (0..=1000).contains(&maximum)
        && (maximum == 0 || maximum >= minimum)
        && valid_string_list(prefixes, 256, 128, true)
        && valid_string_list(suffixes, 256, 128, true)
        && has_prefix_or_suffix
        && ["prefix_color", "suffix_color", "combined_color"]
            .iter()
            .all(|key| string_field(rule, key).is_some_and(valid_hex_color))
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
    if !lists
        .iter()
        .all(|name| filter.get(*name).is_some_and(|value| valid_string_list(value, 256, 128, true)))
    {
        return false;
    }
    let Some(level) =
        filter.get("level").and_then(|level| json_exact_object(level, &["max", "min"]))
    else {
        return false;
    };
    let Some(minimum) = level.get("min").and_then(Value::as_i64) else {
        return false;
    };
    let Some(maximum) = level.get("max").and_then(Value::as_i64) else {
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
        || lists.iter().any(|name| {
            filter.get(*name).and_then(Value::as_array).is_some_and(|values| !values.is_empty())
        })
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
    let (Some(hosts), Some(paths)) =
        (source.get("allowed_hosts"), source.get("allowed_path_prefixes"))
    else {
        return false;
    };
    let hosts_valid = valid_string_list(hosts, 16, 253, true);
    // 后端按前缀的数量和格式校验，单项受整个文档的大小边界约束。
    let paths_valid = valid_string_list(paths, 16, usize::MAX, true);
    let hosts_present = hosts.as_array().is_some_and(|values| !values.is_empty());
    let paths_present = paths.as_array().is_some_and(|values| !values.is_empty());
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::infrastructure::{valid_security, valid_tracing};
    use super::super::operations::tests::default_operations;
    use super::{valid_equipment_source, valid_runtime_document};

    /// 默认可编辑区块的 HTTP 投影，worlds 与装备筛选已配置，历史模板可省略。
    fn default_document() -> Value {
        let world =
            json!({"enabled": true, "sender_allowlist": [], "sender_allowlist_disabled": false});
        json!({
            "allow_manual_early_open": false,
            "allow_manual_early_settlement": false,
            "worlds": { "main": world, "isekai": world },
            "equipment_filter": {
                "enabled": false,
                "qualities": ["Legendary", "Peerless"],
                "prefixes": [],
                "suffixes": [],
                "categories": [],
                "equipment_types": [],
                "level": {"min": 0, "max": 0}
            },
            "equipment_colors": {"rules": []},
            "equipment_source": {
                "allowed_hosts": ["hentaiverse.org"],
                "allowed_path_prefixes": ["/equip/", "/isekai/equip/"],
                "allowlist_enabled": true,
                "require_tradeable": false,
                "maximum_response_bytes": 2097152,
                "minimum_request_interval_millis": 250,
                "request_timeout_seconds": 10
            },
            "listener": {
                "address": "127.0.0.1:8091",
                "group": "",
                "mode": "tcp",
                "runtime_directory": "/run/hv-auction",
                "socket_path": ""
            },
            "dashboard_mailboxes": [],
            "metrics": {"enabled": false},
            "security": {
                "allowed_origins": [],
                "allowed_origins_enabled": true,
                "login_rate_limit": 10,
                "mutation_rate_limit": 60,
                "rate_limit_window_seconds": 60,
                "cookie_secure": false,
                "request_timeout_seconds": 15,
                "trusted_proxy_cidrs": [],
                "trusted_unix_proxy": false
            },
            "session": {"idle_ttl_seconds": 900, "absolute_ttl_seconds": 28800, "max_public_sessions": 10},
            "tracing": {
                "batch_timeout_seconds": 1,
                "enabled": false,
                "endpoint": "",
                "export_timeout_seconds": 5,
                "max_export_batch_size": 128,
                "queue_capacity": 256,
                "sample_ratio_ppm": 0,
                "shutdown_timeout_seconds": 5
            },
            "worker_control": {"socket_path": "/run/hv-auction/worker/control.sock"},
            "operations": default_operations()
        })
    }

    #[test]
    fn runtime_document_accepts_complete_default_projection() {
        assert!(valid_runtime_document(&default_document()));
    }

    #[test]
    fn runtime_document_returns_failure_when_required_keys_are_missing() {
        let mut projection = default_document();
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

    #[test]
    fn equipment_source_accepts_long_path_prefixes() {
        let mut source = default_document()["equipment_source"].clone();
        let prefix = format!("/{}/", "a".repeat(600));
        source["allowed_path_prefixes"] = json!([prefix]);
        assert!(valid_equipment_source(&source));
        source["allowed_path_prefixes"] = json!([prefix, prefix]);
        assert!(!valid_equipment_source(&source));
    }

    #[test]
    fn tracing_accepts_long_http_endpoints() {
        let mut tracing = default_document()["tracing"].clone();
        tracing["enabled"] = json!(true);
        tracing["sample_ratio_ppm"] = json!(1);
        let path = "a".repeat(2100);
        tracing["endpoint"] = json!(format!("https://example.invalid/{path}"));
        assert!(valid_tracing(&tracing));
        tracing["endpoint"] = json!(format!("ftp://example.invalid/{path}"));
        assert!(!valid_tracing(&tracing));
    }

    #[test]
    fn only_enabled_origins_require_entry_validation() {
        let mut security = default_document()["security"].clone();
        security["allowed_origins_enabled"] = json!(false);
        security["allowed_origins"] = json!(["", " https://example.invalid "]);
        assert!(valid_security(&security));
        let mut enabled = security.clone();
        enabled["allowed_origins_enabled"] = json!(true);
        assert!(!valid_security(&enabled));
        security["allowed_origins"] = json!([1]);
        assert!(!valid_security(&security));
        security["allowed_origins"] = json!(vec![""; 17]);
        assert!(!valid_security(&security));
    }
}
