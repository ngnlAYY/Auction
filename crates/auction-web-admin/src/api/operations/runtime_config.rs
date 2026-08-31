//! Admin runtime 配置 HTTP 客户端。
//!
//! 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
//! 文件、密钥和命令账本永远不进入浏览器。

pub(crate) use auction_contracts::AdminRuntimeConfigResponse as AdminRuntimeConfig;
use auction_contracts::{AdminRuntimeConfigUpdateRequest, AdminRuntimeConfigValidateRequest};
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{
    AdminApiFailure, ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error,
    invalid_response_error, response_failure, send_get, send_request,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictRuntimeConfig {
    document: Value,
    fingerprint: String,
    restart_required: bool,
}

/// Runtime 配置请求失败时保留 HTTP 状态，页面可区分 fingerprint 冲突和普通错误。
pub(crate) type AdminRuntimeConfigApiError = AdminApiFailure;

pub(crate) async fn fetch_admin_runtime_config()
-> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let response = send_get("/api/v1/admin/config").await.map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<StrictRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

pub(crate) async fn validate_admin_runtime_config(
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let command = AdminRuntimeConfigValidateRequest { document };
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/config:validate")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<StrictRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, false).map_err(invalid_runtime_response)
}

pub(crate) async fn update_admin_runtime_config(
    command_id: uuid::Uuid,
    expected_fingerprint: String,
    document: Value,
) -> Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
    let csrf_token =
        super::fetch_csrf_token().await.map_err(|code| AdminApiFailure { status: None, code })?;
    let command = AdminRuntimeConfigUpdateRequest { command_id, expected_fingerprint, document };
    let response = send_request(|abort_signal| {
        Request::put("/api/v1/admin/config")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<StrictRuntimeConfig> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_runtime_config(payload.data, true).map_err(invalid_runtime_response)
}

fn decode_runtime_config(
    value: StrictRuntimeConfig,
    expected_restart_required: bool,
) -> Result<AdminRuntimeConfig, &'static str> {
    if value.restart_required != expected_restart_required
        || !valid_fingerprint(&value.fingerprint)
        || !valid_runtime_document(&value.document)
    {
        return Err("runtime_config");
    }
    Ok(AdminRuntimeConfig {
        document: value.document,
        fingerprint: value.fingerprint,
        restart_required: value.restart_required,
    })
}

fn valid_runtime_document(value: &Value) -> bool {
    let Some(document) = exact_object(
        value,
        &[
            "allow_manual_early_open",
            "allow_manual_early_settlement",
            "dashboard_mailboxes",
            "equipment_filter",
            "equipment_source",
            "listener",
            "mail_templates",
            "metrics",
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
        && valid_worlds(&document["worlds"])
        && valid_equipment_filter(&document["equipment_filter"])
        && valid_mail_templates(&document["mail_templates"])
        && valid_equipment_source(&document["equipment_source"])
        && valid_listener(&document["listener"])
        && valid_dashboard_mailboxes(&document["dashboard_mailboxes"])
        && valid_metrics(&document["metrics"])
        && valid_security(&document["security"])
        && valid_session(&document["session"])
        && valid_tracing(&document["tracing"])
        && valid_worker_control(&document["worker_control"])
}

fn valid_worlds(value: &Value) -> bool {
    let Some(worlds) = exact_object(value, &["isekai", "main"]) else {
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

fn valid_runtime_world(value: &Value) -> Option<bool> {
    let world = exact_object(value, &["enabled", "sender_allowlist", "sender_allowlist_disabled"])?;
    let enabled = bool_field(world, "enabled")?;
    bool_field(world, "sender_allowlist_disabled")?;
    valid_string_list(&world["sender_allowlist"], 1000, 128, true).then_some(enabled)
}

fn valid_equipment_filter(value: &Value) -> bool {
    let Some(filter) = exact_object(
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
    let Some(level) = exact_object(&filter["level"], &["max", "min"]) else {
        return false;
    };
    let Some(minimum) = level["min"].as_i64() else {
        return false;
    };
    let Some(maximum) = level["max"].as_i64() else {
        return false;
    };
    if minimum < 0 || maximum < minimum || maximum > 1000 {
        return false;
    }
    !enabled
        || minimum != 0
        || maximum != 0
        || lists
            .iter()
            .any(|name| filter[*name].as_array().is_some_and(|values| !values.is_empty()))
}

fn valid_mail_templates(value: &Value) -> bool {
    const TEMPLATES: [(&str, &[&str], &[&str]); 14] = [
        ("auction_assets_rejected", &[], &[]),
        ("auction_equipment_rejected", &["reason_code"], &["reason_code"]),
        ("auction_no_attachments", &[], &[]),
        ("operations_alert", &["summary"], &["summary"]),
        ("rebind_result", &["result_code"], &["result_code", "username"]),
        ("register_key", &["login_key", "username"], &["login_key", "username"]),
        ("register_result", &["result_code"], &["result_code", "username"]),
        ("settlement_buyer_refund", &["amount", "reference"], &["amount", "reference"]),
        ("settlement_equipment_handoff", &["item_name", "reference"], &["item_name", "reference"]),
        ("settlement_payment_carrier", &["amount", "reference"], &["amount", "reference"]),
        ("settlement_payment_reminder", &["reference"], &["reference"]),
        ("settlement_seller_payout", &["amount", "reference"], &["amount", "reference"]),
        (
            "settlement_winner_equipment_cod",
            &["item_name", "amount", "reference"],
            &["item_name", "amount", "reference"],
        ),
        ("user_assets_rejected", &[], &[]),
    ];
    let keys = TEMPLATES.iter().map(|(name, _, _)| *name).collect::<Vec<_>>();
    let Some(templates) = exact_object(value, &keys) else {
        return false;
    };
    TEMPLATES.iter().all(|(name, required, allowed)| {
        templates[*name].as_str().is_some_and(|body| valid_template(body, required, allowed))
    })
}

fn valid_template(value: &str, required: &[&str], allowed: &[&str]) -> bool {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64 * 1024
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return false;
    }
    let mut cursor = 0;
    let mut seen = Vec::new();
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        let Some(relative_end) = value[start + 1..].find('}') else {
            return false;
        };
        let end = start + 1 + relative_end;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return false;
        }
        if !seen.contains(&placeholder) {
            seen.push(placeholder);
        }
        cursor = end + 1;
    }
    required.iter().all(|placeholder| seen.contains(placeholder))
}

fn valid_equipment_source(value: &Value) -> bool {
    let Some(source) = exact_object(
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

fn valid_listener(value: &Value) -> bool {
    let Some(listener) =
        exact_object(value, &["address", "group", "mode", "runtime_directory", "socket_path"])
    else {
        return false;
    };
    let Some(mode) = string_field(listener, "mode") else {
        return false;
    };
    let Some(address) = string_field(listener, "address") else {
        return false;
    };
    let Some(group) = string_field(listener, "group") else {
        return false;
    };
    let Some(runtime_directory) = string_field(listener, "runtime_directory") else {
        return false;
    };
    let Some(socket_path) = string_field(listener, "socket_path") else {
        return false;
    };
    match mode {
        "tcp" => {
            address.parse::<std::net::SocketAddr>().is_ok()
                && group.is_empty()
                && socket_path.is_empty()
                && valid_path(runtime_directory)
        }
        "unix" => {
            address.is_empty()
                && valid_text(group, 128, false)
                && valid_path(runtime_directory)
                && valid_path(socket_path)
        }
        _ => false,
    }
}

fn valid_dashboard_mailboxes(value: &Value) -> bool {
    let Some(mailboxes) = value.as_array() else {
        return false;
    };
    if mailboxes.len() > 2 {
        return false;
    }
    let mut worlds = Vec::new();
    let mut keys = Vec::new();
    mailboxes.iter().all(|value| {
        let Some(mailbox) = exact_object(value, &["mailbox_key", "world_id"]) else {
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

fn valid_metrics(value: &Value) -> bool {
    exact_object(value, &["enabled"]).and_then(|metrics| bool_field(metrics, "enabled")).is_some()
}

fn valid_security(value: &Value) -> bool {
    let Some(security) = exact_object(
        value,
        &[
            "allowed_origins",
            "allowed_origins_enabled",
            "cookie_secure",
            "login_rate_limit",
            "mutation_rate_limit",
            "rate_limit_window_seconds",
            "request_timeout_seconds",
            "trusted_proxy_cidrs",
            "trusted_unix_proxy",
        ],
    ) else {
        return false;
    };
    bool_field(security, "allowed_origins_enabled").is_some()
        && bool_field(security, "cookie_secure").is_some()
        && bool_field(security, "trusted_unix_proxy").is_some()
        && valid_string_list(&security["allowed_origins"], 16, 512, false)
        && valid_string_list(&security["trusted_proxy_cidrs"], 64, 128, true)
        && u64_field(security, "login_rate_limit")
            .is_some_and(|value| (1..=10_000).contains(&value))
        && u64_field(security, "mutation_rate_limit")
            .is_some_and(|value| (1..=10_000).contains(&value))
        && u64_field(security, "rate_limit_window_seconds")
            .is_some_and(|value| (1..=3600).contains(&value))
        && u64_field(security, "request_timeout_seconds")
            .is_some_and(|value| (1..=120).contains(&value))
}

fn valid_session(value: &Value) -> bool {
    let Some(session) =
        exact_object(value, &["absolute_ttl_seconds", "idle_ttl_seconds", "max_public_sessions"])
    else {
        return false;
    };
    let Some(idle) = u64_field(session, "idle_ttl_seconds") else {
        return false;
    };
    let Some(absolute) = u64_field(session, "absolute_ttl_seconds") else {
        return false;
    };
    let Some(maximum) = u64_field(session, "max_public_sessions") else {
        return false;
    };
    (60..=86_400).contains(&idle)
        && (idle..=2_592_000).contains(&absolute)
        && (1..=100).contains(&maximum)
}

fn valid_tracing(value: &Value) -> bool {
    let Some(tracing) = exact_object(
        value,
        &[
            "batch_timeout_seconds",
            "enabled",
            "endpoint",
            "export_timeout_seconds",
            "max_export_batch_size",
            "queue_capacity",
            "sample_ratio_ppm",
            "shutdown_timeout_seconds",
        ],
    ) else {
        return false;
    };
    let Some(enabled) = bool_field(tracing, "enabled") else {
        return false;
    };
    let Some(endpoint) = string_field(tracing, "endpoint") else {
        return false;
    };
    let Some(batch) = u64_field(tracing, "max_export_batch_size") else {
        return false;
    };
    let Some(queue) = u64_field(tracing, "queue_capacity") else {
        return false;
    };
    u64_field(tracing, "batch_timeout_seconds").is_some_and(|value| (1..=30).contains(&value))
        && u64_field(tracing, "export_timeout_seconds")
            .is_some_and(|value| (1..=30).contains(&value))
        && u64_field(tracing, "shutdown_timeout_seconds")
            .is_some_and(|value| (1..=30).contains(&value))
        && (1..=512).contains(&batch)
        && (1..=4096).contains(&queue)
        && batch <= queue
        && u64_field(tracing, "sample_ratio_ppm").is_some_and(|value| value <= 1_000_000)
        && (!enabled && endpoint.is_empty()
            || (endpoint.starts_with("http://") || endpoint.starts_with("https://"))
                && valid_text(endpoint, 2048, false))
}

fn valid_worker_control(value: &Value) -> bool {
    exact_object(value, &["socket_path"])
        .and_then(|control| string_field(control, "socket_path"))
        .is_some_and(valid_path)
}

fn exact_object<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    (object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key)))
        .then_some(object)
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key)?.as_bool()
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key)?.as_u64().filter(|value| *value <= MAX_JSON_SAFE_INTEGER as u64)
}

fn valid_string_list(
    value: &Value,
    maximum_count: usize,
    maximum_bytes: usize,
    unique: bool,
) -> bool {
    let Some(values) = value.as_array() else {
        return false;
    };
    values.len() <= maximum_count
        && values.iter().enumerate().all(|(index, value)| {
            value.as_str().is_some_and(|text| {
                valid_text(text, maximum_bytes, false)
                    && (!unique
                        || values[..index].iter().all(|previous| previous.as_str() != Some(text)))
            })
        })
}

fn valid_text(value: &str, maximum_bytes: usize, allow_empty: bool) -> bool {
    (allow_empty || !value.is_empty())
        && value.len() <= maximum_bytes
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.starts_with('/')
        && value != "/"
        && value == value.trim()
        && !value.chars().any(char::is_control)
        && !value.contains("//")
        && value.split('/').all(|segment| segment != "." && segment != "..")
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn invalid_runtime_response(_error: &'static str) -> AdminRuntimeConfigApiError {
    AdminApiFailure::client("invalid_response")
}
