//! 运行配置基础设施区块的响应结构校验。

use serde_json::Value;

use super::fields::{valid_scalars, valid_string_array};

pub(super) fn valid_listener(value: &Value) -> bool {
    valid_scalars(
        value,
        &[],
        &["address", "group", "mode", "runtime_directory", "socket_path"],
        &[],
        &[],
    ) && matches!(value["mode"].as_str(), Some("tcp" | "unix"))
}

pub(super) fn valid_metrics(value: &Value) -> bool {
    valid_scalars(value, &["enabled"], &[], &[], &[])
}

pub(super) fn valid_security(value: &Value) -> bool {
    valid_scalars(
        value,
        &["allowed_origins_enabled", "cookie_secure", "trusted_unix_proxy"],
        &[],
        &[
            "login_rate_limit",
            "mutation_rate_limit",
            "rate_limit_window_seconds",
            "request_timeout_seconds",
        ],
        &["allowed_origins", "trusted_proxy_cidrs"],
    ) && valid_string_array(&value["allowed_origins"])
        && valid_string_array(&value["trusted_proxy_cidrs"])
}

pub(super) fn valid_session(value: &Value) -> bool {
    valid_scalars(
        value,
        &[],
        &[],
        &["absolute_ttl_seconds", "idle_ttl_seconds", "max_public_sessions"],
        &[],
    )
}

pub(super) fn valid_tracing(value: &Value) -> bool {
    valid_scalars(
        value,
        &["enabled"],
        &["endpoint"],
        &[
            "batch_timeout_seconds",
            "export_timeout_seconds",
            "max_export_batch_size",
            "queue_capacity",
            "sample_ratio_ppm",
            "shutdown_timeout_seconds",
        ],
        &[],
    )
}

pub(super) fn valid_worker_control(value: &Value) -> bool {
    valid_scalars(value, &[], &["socket_path"], &[], &[])
}
