//! 新运行参数的严格 JSON 字段闭集与数值边界；关联业务校验仍由服务端文档 owner 完成。

use serde_json::Value;

use super::{bool_field, exact_object, string_field, u64_field, valid_string_list, valid_text};

pub(super) fn valid_operations(value: &Value) -> bool {
    let Some(document) =
        exact_object(value, &["economics", "schedule", "forum", "alerts", "worker", "mail"])
    else {
        return false;
    };
    valid_economics(&document["economics"])
        && valid_schedule(&document["schedule"])
        && valid_forum(&document["forum"])
        && valid_alerts(&document["alerts"])
        && valid_worker(&document["worker"])
        && valid_mail(&document["mail"])
}

fn valid_mail(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if !object.contains_key("batch_timeout_seconds") {
        let mut normalized = object.clone();
        normalized.insert("batch_timeout_seconds".to_owned(), Value::from(600_u64));
        return valid_mail(&Value::Object(normalized));
    }
    let classes_incomplete =
        object.get("send_classes").and_then(Value::as_object).is_some_and(|classes| {
            [
                "account_reply",
                "intake_reply",
                "settlement_notice",
                "outbid_reminder",
                "player_summary",
                "operations_alert",
            ]
            .iter()
            .any(|name| !classes.contains_key(*name))
        });
    if !object.contains_key("send_intervals")
        || !object.contains_key("send_classes")
        || classes_incomplete
    {
        let mut normalized = object.clone();
        normalized.entry("send_intervals".to_owned()).or_insert_with(|| {
            serde_json::json!({
                "outbid_seconds": 0,
                "account_seconds": 0,
                "intake_seconds": 0,
                "settlement_seconds": 0,
                "equipment_seconds": 0,
                "operations_seconds": 0,
                "manual_seconds": 0
            })
        });
        let classes = normalized
            .entry("send_classes".to_owned())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(classes) = classes.as_object_mut() {
            // 部分键按服务端 serde 默认值补 true；前端校验与保存结构保持一致。
            for name in [
                "account_reply",
                "intake_reply",
                "settlement_notice",
                "outbid_reminder",
                "player_summary",
                "operations_alert",
            ] {
                classes.entry(name.to_owned()).or_insert(Value::Bool(true));
            }
        }
        return valid_mail(&Value::Object(normalized));
    }
    // 旧 runtime 文档没有该字段；缺省时按启动默认值校验，未编辑保存不写回。
    let reply_defaults = [
        ("rekey_enabled", Value::Bool(true)),
        ("account_reply_window_seconds", Value::from(86_400_u64)),
        ("explanation_mail_window_seconds", Value::from(86_400_u64)),
        ("explanation_mail_limit", Value::from(10_u64)),
    ];
    if reply_defaults.iter().any(|(name, _)| !object.contains_key(*name)) {
        let mut normalized = object.clone();
        for (name, value) in reply_defaults {
            normalized.entry(name.to_owned()).or_insert(value);
        }
        return valid_mail(&Value::Object(normalized));
    }
    valid_scalars(
        value,
        &["managed", "rekey_enabled"],
        &[("schedule", false)],
        &[
            ("maximum_response_bytes", 1, 8_388_608),
            ("minimum_request_interval_millis", 1, 60_000),
            ("request_timeout_seconds", 1, 300),
            ("batch_size", 1, 1000),
            ("maximum_pages", 1, 100),
            ("jitter_seconds", 0, 86_400),
            ("lease_duration_seconds", 1, 86_400),
            ("renew_interval_seconds", 1, 86_399),
            ("fetch_attempts", 1, 10),
            ("fetch_timeout_seconds", 1, 300),
            ("process_attempts", 1, 10),
            ("item_timeout_seconds", 1, 300),
            ("batch_timeout_seconds", 1, 86_400),
            ("retry_base_seconds", 1, 300),
            ("retry_maximum_seconds", 1, 300),
            ("account_reply_window_seconds", 60, 604_800),
            ("explanation_mail_window_seconds", 60, 604_800),
            ("explanation_mail_limit", 0, 100),
        ],
        &["send_intervals", "send_classes"],
    ) && valid_send_intervals(&value["send_intervals"])
        && valid_send_classes(&value["send_classes"])
        && value["renew_interval_seconds"].as_u64() < value["lease_duration_seconds"].as_u64()
        && value["fetch_timeout_seconds"].as_u64() > value["request_timeout_seconds"].as_u64()
        && value["item_timeout_seconds"].as_u64() > value["request_timeout_seconds"].as_u64()
        && value["batch_timeout_seconds"].as_u64().unwrap_or_default()
            >= value["fetch_timeout_seconds"]
                .as_u64()
                .unwrap_or_default()
                .saturating_add(value["item_timeout_seconds"].as_u64().unwrap_or_default())
        && value["retry_base_seconds"].as_u64() <= value["retry_maximum_seconds"].as_u64()
}

fn valid_send_intervals(value: &Value) -> bool {
    valid_scalars(
        value,
        &[],
        &[],
        &[
            ("outbid_seconds", 0, 86_400),
            ("account_seconds", 0, 86_400),
            ("intake_seconds", 0, 86_400),
            ("settlement_seconds", 0, 86_400),
            ("equipment_seconds", 0, 86_400),
            ("operations_seconds", 0, 86_400),
            ("manual_seconds", 0, 86_400),
        ],
        &[],
    )
}

fn valid_send_classes(value: &Value) -> bool {
    let Some(object) = exact_object(
        value,
        &[
            "account_reply",
            "intake_reply",
            "settlement_notice",
            "outbid_reminder",
            "player_summary",
            "operations_alert",
        ],
    ) else {
        return false;
    };
    object.values().all(Value::is_boolean)
}

fn valid_economics(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &["managed", "proxy_enabled"],
        &[("proxy_default_mode", false)],
        &[
            ("start_price", 1000, 1000000000),
            ("bid_increment_ppm", 0, 1000000),
            ("soft_close_window_seconds", 0, 604800),
            ("soft_close_extension_seconds", 1, 604800),
            ("cancel_cutoff_seconds", 0, 604800),
            ("proxy_slow_interval_seconds", 1, 604800),
            ("proxy_max_outbid_uses", 0, 1000),
            ("final_phase_lead_seconds", 1, 604800),
        ],
        &[],
    ) {
        return false;
    }
    matches!(value["proxy_default_mode"].as_str(), Some("fast" | "slow"))
}

fn valid_schedule(value: &Value) -> bool {
    // 早鸟区块可省略；直接校验借用字段，不复制整份排期后递归移除可选区块。
    let nested: &[&str] = if let Some(early_bird) = value.get("early_bird") {
        if !valid_early_bird(early_bird) {
            return false;
        }
        &["worlds", "early_bird"]
    } else {
        &["worlds"]
    };
    if !valid_scalars(
        value,
        &["managed", "enabled"],
        &[
            ("timezone", false),
            ("preview_day", false),
            ("preview_time", false),
            ("open_day", false),
            ("open_time", false),
            ("nominal_end_day", false),
            ("nominal_end_time", false),
        ],
        &[],
        nested,
    ) {
        return false;
    }
    valid_string_list(&value["worlds"], 2, 16, true)
        && value["worlds"].as_array().is_some_and(|worlds| {
            !worlds.is_empty()
                && worlds.iter().all(|world| matches!(world.as_str(), Some("main" | "isekai")))
        })
        && ["preview_day", "open_day", "nominal_end_day"].iter().all(|name| {
            matches!(
                value[*name].as_str(),
                Some(
                    "Monday"
                        | "Tuesday"
                        | "Wednesday"
                        | "Thursday"
                        | "Friday"
                        | "Saturday"
                        | "Sunday"
                )
            )
        })
}

fn valid_early_bird(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &["enabled", "pause_intake_before_season"],
        &[("timezone", false), ("season_start_time", false), ("open_time", false)],
        &[
            ("window_days", 1, 31),
            ("preview_lead_minutes", 120, 10_080),
            ("live_minutes", 1, 10_080),
        ],
        &["worlds", "season_starts", "session_days", "qualities"],
    ) {
        return false;
    }
    valid_string_list(&value["worlds"], 2, 16, true)
        && value["worlds"].as_array().is_some_and(|worlds| {
            !worlds.is_empty()
                && worlds.iter().all(|world| matches!(world.as_str(), Some("main" | "isekai")))
        })
        && valid_string_list(&value["season_starts"], 12, 5, true)
        && value["season_starts"].as_array().is_some_and(|starts| !starts.is_empty())
        && value["session_days"].as_array().is_some_and(|days| {
            !days.is_empty()
                && days.len() <= 31
                && days.iter().all(|day| day.as_u64().is_some_and(|day| (1..=31).contains(&day)))
        })
        && valid_string_list(&value["qualities"], 256, 128, true)
}

fn valid_forum(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &["managed", "enabled"],
        &[],
        &[
            ("batch_size", 1, 100),
            ("poll_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("maximum_response_bytes", 1, 8388608),
            ("maximum_topic_pages", 1, 20),
            ("minimum_request_interval_millis", 1, 60000),
            ("request_timeout_seconds", 1, 300),
            ("item_timeout_seconds", 1, 3600),
            ("lease_duration_seconds", 1, 86400),
        ],
        &[],
    ) {
        return false;
    }
    true
}

fn valid_alerts(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &["managed", "enabled"],
        &[("world_id", false), ("recipient", true)],
        &[
            ("scan_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("source_timeout_seconds", 1, 30),
            ("batch_window_seconds", 1, 3600),
            ("cooldown_seconds", 1, 604800),
            ("claim_lease_seconds", 1, 86400),
            ("retry_delay_seconds", 1, 86400),
            ("max_signals_per_scan", 1, 64),
            ("max_batch_items", 1, 64),
            ("max_body_bytes", 256, 16384),
            ("delivery_pending_count_threshold", 1, 9007199254740991),
            ("delivery_pending_age_seconds", 1, 2592000),
            ("delivery_manual_reconcile_count_threshold", 1, 9007199254740991),
            ("fulfillment_backlog_count_threshold", 1, 9007199254740991),
            ("fulfillment_backlog_age_seconds", 1, 2592000),
            ("recovery_overdue_count_threshold", 1, 9007199254740991),
            ("recovery_overdue_age_seconds", 1, 2592000),
        ],
        &[],
    ) {
        return false;
    }
    matches!(value["world_id"].as_str(), Some("main" | "isekai"))
}

fn valid_worker(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &["managed"],
        &[],
        &[
            ("concurrency", 1, 1000),
            ("poll_interval_seconds", 1, 86400),
            ("batch_limit", 1, 1000),
            ("jitter_seconds", 0, 86400),
            ("item_timeout_seconds", 1, 86400),
            ("recovery_retry_delay_seconds", 1, 86400),
        ],
        &["delivery", "fulfillment", "recovery"],
    ) {
        return false;
    }
    valid_worker_delivery(&value["delivery"])
        && valid_worker_fulfillment(&value["fulfillment"])
        && valid_worker_recovery(&value["recovery"])
}

fn valid_worker_delivery(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &[],
        &[],
        &[
            ("batch_limit", 1, 1000),
            ("concurrency", 1, 1000),
            ("poll_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("lease_duration_seconds", 1, 86400),
            ("renew_interval_seconds", 1, 86400),
            ("sending_grace_seconds", 1, 86400),
            ("request_timeout_seconds", 1, 86400),
            ("finalize_timeout_seconds", 1, 300),
        ],
        &[],
    ) {
        return false;
    }
    true
}

fn valid_worker_fulfillment(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &[],
        &[],
        &[
            ("batch_limit", 1, 1000),
            ("concurrency", 1, 1000),
            ("poll_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("lease_duration_seconds", 1, 86400),
            ("item_timeout_seconds", 1, 86400),
            ("retry_delay_seconds", 1, 86400),
        ],
        &[],
    ) {
        return false;
    }
    true
}

fn valid_worker_recovery(value: &Value) -> bool {
    if !valid_scalars(
        value,
        &[],
        &[],
        &[
            ("batch_limit", 1, 1000),
            ("concurrency", 1, 1000),
            ("poll_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("lease_duration_seconds", 1, 86400),
            ("item_timeout_seconds", 1, 86400),
            ("low_balance_threshold", 0, 9007199254740991),
            ("health_refresh_interval_seconds", 1, 86400),
            ("evidence_stale_after_seconds", 1, 86400),
        ],
        &[],
    ) {
        return false;
    }
    true
}

fn valid_scalars(
    value: &Value,
    booleans: &[&str],
    strings: &[(&str, bool)],
    numbers: &[(&str, u64, u64)],
    nested: &[&str],
) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == booleans.len() + strings.len() + numbers.len() + nested.len()
        && nested.iter().all(|name| object.contains_key(*name))
        && booleans.iter().all(|name| bool_field(object, name).is_some())
        && strings.iter().all(|(name, empty)| {
            string_field(object, name).is_some_and(|value| valid_text(value, 256, *empty))
        })
        && numbers.iter().all(|(name, minimum, maximum)| {
            u64_field(object, name).is_some_and(|value| (*minimum..=*maximum).contains(&value))
        })
}
