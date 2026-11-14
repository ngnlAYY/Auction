//! Dashboard 响应的字段闭集校验；页面只接受后端 owner 构造的有界 JSON 形状。

use auction_web_shared::wire::{json_exact_keys, json_exact_object};
use serde_json::Value;

pub(super) fn valid_dashboard_shape(value: &Value) -> bool {
    let Some(dashboard) = json_exact_object(
        value,
        &[
            "health",
            "worker",
            "process",
            "mail",
            "realtime",
            "credentials",
            "external_accounts",
            "delivery",
            "fulfillment",
            "recovery",
            "alerting",
            "reconciliation",
        ],
    ) else {
        return false;
    };

    valid_health_shape(&dashboard["health"])
        && json_exact_object(
            &dashboard["worker"],
            &["condition", "mail_pollers", "ready", "state", "version"],
        )
        .is_some()
        && valid_section(&dashboard["process"], valid_process_shape, true)
        && valid_section(&dashboard["mail"], valid_mail_shape, true)
        && valid_section(&dashboard["realtime"], valid_realtime_shape, true)
        && valid_section(&dashboard["credentials"], valid_credentials_shape, true)
        && valid_section(&dashboard["external_accounts"], valid_external_accounts_shape, true)
        && valid_section(&dashboard["delivery"], valid_delivery_shape, true)
        && valid_section(&dashboard["fulfillment"], valid_fulfillment_shape, true)
        && valid_section(&dashboard["recovery"], valid_recovery_shape, true)
        && valid_alerting_section(&dashboard["alerting"])
        && valid_section(&dashboard["reconciliation"], valid_reconciliation_shape, true)
}

fn valid_reconciliation_shape(value: &Value) -> bool {
    let Some(data) = value.as_object() else {
        return false;
    };
    data.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>()
        == ["custody_report_age_seconds", "observed_at", "solvency_report_age_seconds", "worlds"]
            .into_iter()
            .collect()
        && data["worlds"].as_array().is_some_and(|worlds| {
            worlds.len() <= 2
                && worlds.iter().all(|world| {
                    json_exact_object(
                        world,
                        &["custody_missing_count", "solvency_shortfall_credits", "world"],
                    )
                    .is_some()
                })
        })
}

fn valid_section(
    value: &Value,
    valid_data: fn(&Value) -> bool,
    observed_at_required: bool,
) -> bool {
    let Some(section) = value.as_object() else {
        return false;
    };
    match section.get("status").and_then(Value::as_str) {
        Some("unknown") => {
            json_exact_keys(section, &["error_code", "status"])
                && section
                    .get("error_code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| !code.is_empty())
        }
        Some("ok" | "degraded") => {
            let keys_valid = if observed_at_required {
                json_exact_keys(section, &["data", "observed_at", "status"])
                    && section.get("observed_at").is_some_and(Value::is_string)
            } else {
                (section.len() == 2 || section.len() == 3)
                    && section.contains_key("data")
                    && section.contains_key("status")
                    && section.get("observed_at").is_none_or(Value::is_string)
            };
            keys_valid && section.get("data").is_some_and(valid_data)
        }
        _ => false,
    }
}

fn valid_external_accounts_shape(value: &Value) -> bool {
    let Some(data) =
        json_exact_object(value, &["credentials", "forum", "mail", "maintenance", "observed_at"])
    else {
        return false;
    };
    let Some(mail) = json_exact_object(
        &data["mail"],
        &[
            "busy",
            "busy_age_seconds",
            "gate_wait_count",
            "gate_wait_seconds_total",
            "last_failure_at",
            "last_request_at",
            "last_success_at",
            "pollers",
            "request_count",
        ],
    ) else {
        return false;
    };
    let Some(forum) = json_exact_object(
        &data["forum"],
        &["last_failure_at", "last_request_at", "last_success_at", "write_unknown_count"],
    ) else {
        return false;
    };
    let Some(maintenance) = json_exact_object(
        &data["maintenance"],
        &[
            "last_canceled_at",
            "last_failure_at",
            "last_started_at",
            "last_success_at",
            "running_age_seconds",
            "state",
        ],
    ) else {
        return false;
    };
    let busy_age_consistent = match (mail["busy"].as_bool(), mail["busy_age_seconds"].as_i64()) {
        (Some(true), Some(age)) => age >= 0,
        (Some(false), None) => true,
        _ => false,
    };
    let maintenance_state_valid = matches!(
        maintenance["state"].as_str(),
        Some("idle" | "running" | "succeeded" | "failed" | "canceled")
    );
    busy_age_consistent
        && maintenance_state_valid
        && valid_external_projection(&mail["pollers"], |poller| {
            json_exact_object(
                poller,
                &[
                    "consecutive_failures",
                    "last_poll_at",
                    "mailbox_key",
                    "next_poll_at",
                    "paused",
                    "paused_since",
                ],
            )
            .is_some()
        })
        && valid_external_projection(&data["credentials"], |credential| {
            json_exact_object(credential, &["available", "invalid", "kind", "world"]).is_some()
        })
        && forum["write_unknown_count"].as_i64().is_some_and(|count| count >= 0)
}

/// 子投影只允许 `available|unavailable`；不可用时空列表不是“没有条目”的事实。
fn valid_external_projection(value: &Value, valid_item: fn(&Value) -> bool) -> bool {
    let Some(projection) = json_exact_object(value, &["items", "status"]) else {
        return false;
    };
    matches!(projection["status"].as_str(), Some("available" | "unavailable"))
        && projection["items"].as_array().is_some_and(|items| items.iter().all(valid_item))
}

fn valid_health_shape(value: &Value) -> bool {
    json_exact_object(value, &["audit_failure_count", "missing_services", "readiness", "status"])
        .is_some()
}

fn valid_process_shape(value: &Value) -> bool {
    let Some(data) = json_exact_object(value, &["observed_at", "processes", "summary"]) else {
        return false;
    };
    data["processes"].as_array().is_some_and(|processes| {
        processes.iter().all(|process| {
            let Some(process) = json_exact_object(
                process,
                &[
                    "audit_failure_count",
                    "expires_at",
                    "instance_id",
                    "observed_at",
                    "process_role",
                    "readiness",
                    "service_name",
                    "started_at",
                    "status",
                    "tracing",
                    "version",
                ],
            ) else {
                return false;
            };
            json_exact_object(
                &process["tracing"],
                &[
                    "dropped_span_count",
                    "export_failure_count",
                    "last_error_code",
                    "last_failure_at",
                    "last_success_at",
                    "status",
                ],
            )
            .is_some()
        })
    }) && valid_health_shape(&data["summary"])
}

fn valid_mail_shape(value: &Value) -> bool {
    let Some(data) = json_exact_object(value, &["observed_at", "pollers"]) else {
        return false;
    };
    data["pollers"].as_array().is_some_and(|pollers| {
        pollers.iter().all(|poller| {
            json_exact_object(
                poller,
                &[
                    "consecutive_failures",
                    "last_poll_at",
                    "mailbox_key",
                    "next_poll_at",
                    "paused",
                    "state_version",
                    "updated_at",
                ],
            )
            .is_some()
        })
    })
}

fn valid_realtime_shape(value: &Value) -> bool {
    let Some(data) = json_exact_object(value, &["failure", "lags", "observed_at", "total_lag"])
    else {
        return false;
    };
    json_exact_object(&data["failure"], &["count", "latest_code", "oldest_at"]).is_some()
        && data["total_lag"].as_i64().is_some_and(|total_lag| total_lag >= 0)
        && data["lags"].as_array().is_some_and(|lags| {
            lags.iter().all(|lag| {
                json_exact_object(
                    lag,
                    &[
                        "consumer_key",
                        "lag",
                        "last_advanced_at",
                        "latest_sequence",
                        "offset",
                        "stream_key",
                    ],
                )
                .is_some()
            })
        })
}

fn valid_credentials_shape(value: &Value) -> bool {
    let Some(data) =
        json_exact_object(value, &["availability", "last_admin_change_at", "observed_at"])
    else {
        return false;
    };
    data["availability"].as_array().is_some_and(|availability| {
        availability.iter().all(|item| {
            json_exact_object(
                item,
                &["available", "invalid", "restart_required", "updated_at", "world"],
            )
            .is_some()
        })
    })
}

fn valid_delivery_shape(value: &Value) -> bool {
    let Some(data) = json_exact_object(value, &["observed_at", "water"]) else {
        return false;
    };
    json_exact_object(
        &data["water"],
        &[
            "failed",
            "last_update_at",
            "manual_reconcile",
            "oldest_manual_reconcile_age_seconds",
            "oldest_pending_age_seconds",
            "pending",
        ],
    )
    .is_some()
}

fn valid_fulfillment_shape(value: &Value) -> bool {
    json_exact_object(
        value,
        &[
            "blocked",
            "failed",
            "manual_reconcile",
            "observed_at",
            "oldest_backlog_age_seconds",
            "oldest_prerequisite_wait_age_seconds",
            "pending",
            "waiting_payment",
            "waiting_prerequisite",
        ],
    )
    .is_some()
}

fn valid_recovery_shape(value: &Value) -> bool {
    let Some(data) = json_exact_object(
        value,
        &[
            "failed_action_count",
            "manual_reconcile_count",
            "observed_at",
            "oldest_overdue_age_seconds",
            "payment_overdue_count",
            "relist_pending_count",
            "worlds",
        ],
    ) else {
        return false;
    };
    data["worlds"].as_array().is_some_and(|worlds| {
        worlds.iter().all(|world| {
            json_exact_object(
                world,
                &[
                    "balance_credits",
                    "balance_observed_at",
                    "balance_status",
                    "evidence_last_success_at",
                    "evidence_status",
                    "updated_at",
                    "world",
                ],
            )
            .is_some()
        })
    })
}

fn valid_alerting_section(value: &Value) -> bool {
    if !valid_section(value, valid_alerting_shape, false) {
        return false;
    }
    let Some(section) = value.as_object() else {
        return false;
    };
    if section.get("status").and_then(Value::as_str) == Some("unknown") {
        return true;
    }
    let Some(data) = section.get("data").and_then(Value::as_object) else {
        return false;
    };
    if data.get("enabled").and_then(Value::as_bool) == Some(false) {
        return section.get("status").and_then(Value::as_str) == Some("ok")
            && !section.contains_key("observed_at");
    }
    match data.get("last_observed_at").and_then(Value::as_str) {
        Some(observed_at) => {
            section.get("observed_at").and_then(Value::as_str) == Some(observed_at)
        }
        None => !section.contains_key("observed_at"),
    }
}

fn valid_alerting_shape(value: &Value) -> bool {
    json_exact_object(
        value,
        &[
            "active_signal_count",
            "claimed_batch_count",
            "due_signal_count",
            "enabled",
            "failed_batch_count",
            "last_error_code",
            "last_failure_at",
            "last_observed_at",
            "last_success_at",
            "manual_reconcile_count",
            "succeeded_batch_count",
            "webhook_failed_count",
            "webhook_manual_reconcile_count",
            "webhook_pending_count",
            "webhook_sending_count",
        ],
    )
    .is_some()
}
