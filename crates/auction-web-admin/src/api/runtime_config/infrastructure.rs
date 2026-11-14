//! 运行配置文档的基础设施段校验：监听、指标、HTTP 安全、会话、tracing 与 Worker 控制面。

use auction_web_shared::wire::json_exact_object;
use serde_json::Value;

use super::fields::{
    bool_field, string_field, u64_field, valid_path, valid_string_list, valid_text,
};

pub(super) fn valid_listener(value: &Value) -> bool {
    let Some(listener) =
        json_exact_object(value, &["address", "group", "mode", "runtime_directory", "socket_path"])
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
                // TCP 不使用 Unix runtime 目录，必须接受服务端允许的空值或普通文本。
                && valid_text(runtime_directory, 512, true)
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

pub(super) fn valid_metrics(value: &Value) -> bool {
    json_exact_object(value, &["enabled"])
        .and_then(|metrics| bool_field(metrics, "enabled"))
        .is_some()
}

pub(super) fn valid_security(value: &Value) -> bool {
    let Some(security) = json_exact_object(
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

pub(super) fn valid_session(value: &Value) -> bool {
    let Some(session) = json_exact_object(
        value,
        &["absolute_ttl_seconds", "idle_ttl_seconds", "max_public_sessions"],
    ) else {
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

pub(super) fn valid_tracing(value: &Value) -> bool {
    let Some(tracing) = json_exact_object(
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

pub(super) fn valid_worker_control(value: &Value) -> bool {
    json_exact_object(value, &["socket_path"])
        .and_then(|control| string_field(control, "socket_path"))
        .is_some_and(valid_path)
}
