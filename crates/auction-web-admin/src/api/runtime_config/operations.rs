//! 新运行参数的严格 JSON 字段闭集与数值边界；关联业务校验仍由服务端文档 owner 完成。

use auction_web_shared::wire::json_exact_object;
use serde_json::Value;

use super::fields::bool_field;
use super::fields::string_field;
use super::fields::u64_field;
use super::fields::valid_string_list;
use super::fields::valid_text;

pub(super) fn valid_operations(value: &Value) -> bool {
    let Some(document) =
        json_exact_object(value, &["economics", "schedule", "forum", "alerts", "worker", "mail"])
    else {
        return false;
    };
    document.get("economics").is_some_and(valid_economics)
        && document.get("schedule").is_some_and(valid_schedule)
        && document.get("forum").is_some_and(valid_forum)
        && document.get("alerts").is_some_and(valid_alerts)
        && document.get("worker").is_some_and(valid_worker)
        && document.get("mail").is_some_and(valid_mail)
}

/// 服务端投影已补齐邮件区块的全部默认值；这里只校验完整闭集与数值边界，缺键即拒绝。
fn valid_mail(value: &Value) -> bool {
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
    let Some(object) = json_exact_object(
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
    let mut required = vec!["worlds", "season_starts", "session_days", "qualities"];
    // 后端省略赛季前场次的默认值，两键各自出现时仍须符合类型与取值边界。
    if let Some(session) = value.get("pre_season_session") {
        if !session.is_boolean() {
            return false;
        }
        required.push("pre_season_session");
    }
    if let Some(days) = value.get("pre_season_days_before") {
        if !days.as_u64().is_some_and(|days| (1..=31).contains(&days)) {
            return false;
        }
        required.push("pre_season_days_before");
    }
    if !valid_scalars(
        value,
        &["enabled", "pause_intake_before_season"],
        &[("timezone", false), ("season_start_time", false), ("open_time", false)],
        &[
            ("window_days", 1, 31),
            ("preview_lead_minutes", 120, 10_080),
            ("live_minutes", 1, 10_080),
        ],
        &required,
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
    valid_scalars(
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
    )
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
            ("fulfillment_collection_overdue_age_seconds", 1, 2592000),
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
    valid_scalars(
        value,
        &[],
        &[],
        &[
            ("batch_limit", 1, 1000),
            ("poll_interval_seconds", 1, 86400),
            ("jitter_seconds", 0, 86400),
            ("lease_duration_seconds", 1, 86400),
            ("renew_interval_seconds", 1, 86400),
            ("sending_grace_seconds", 1, 86400),
            ("request_timeout_seconds", 1, 86400),
            ("finalize_timeout_seconds", 1, 300),
        ],
        &[],
    )
}

fn valid_worker_fulfillment(value: &Value) -> bool {
    valid_scalars(
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
    )
}

fn valid_worker_recovery(value: &Value) -> bool {
    valid_scalars(
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
    )
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

#[cfg(test)]
pub(super) mod tests {
    use serde_json::{Value, json};

    use super::{valid_alerts, valid_early_bird, valid_mail, valid_operations};

    /// 各后端 Default 经当前 HTTP 扩展投影后的运行参数。
    pub(in crate::api::runtime_config) fn default_operations() -> Value {
        json!({
            "economics": {
                "managed": false,
                "start_price": 1000,
                "bid_increment_ppm": 50000,
                "soft_close_window_seconds": 900,
                "soft_close_extension_seconds": 900,
                "cancel_cutoff_seconds": 86400,
                "proxy_enabled": true,
                "proxy_default_mode": "fast",
                "proxy_slow_interval_seconds": 600,
                "proxy_max_outbid_uses": 3,
                "final_phase_lead_seconds": 900
            },
            "schedule": {
                "managed": false,
                "enabled": false,
                "timezone": "UTC",
                "preview_day": "Thursday",
                "preview_time": "22:00",
                "open_day": "Friday",
                "open_time": "00:00",
                "nominal_end_day": "Saturday",
                "nominal_end_time": "12:00",
                "worlds": ["main", "isekai"]
            },
            "forum": {
                "managed": false,
                "enabled": false,
                "batch_size": 100,
                "poll_interval_seconds": 300,
                "jitter_seconds": 5,
                "maximum_response_bytes": 2097152,
                "maximum_topic_pages": 10,
                "minimum_request_interval_millis": 750,
                "request_timeout_seconds": 30,
                "item_timeout_seconds": 240,
                "lease_duration_seconds": 300
            },
            "alerts": complete_alerts(),
            "worker": {
                "managed": false,
                "concurrency": 8,
                "poll_interval_seconds": 60,
                "batch_limit": 100,
                "jitter_seconds": 5,
                "item_timeout_seconds": 30,
                "recovery_retry_delay_seconds": 60,
                "delivery": {
                    "batch_limit": 100,
                    "poll_interval_seconds": 60,
                    "jitter_seconds": 5,
                    "lease_duration_seconds": 60,
                    "renew_interval_seconds": 10,
                    "sending_grace_seconds": 300,
                    "request_timeout_seconds": 180,
                    "finalize_timeout_seconds": 5
                },
                "fulfillment": {
                    "batch_limit": 100,
                    "concurrency": 8,
                    "poll_interval_seconds": 60,
                    "jitter_seconds": 5,
                    "lease_duration_seconds": 60,
                    "item_timeout_seconds": 30,
                    "retry_delay_seconds": 60
                },
                "recovery": {
                    "batch_limit": 100,
                    "concurrency": 8,
                    "poll_interval_seconds": 60,
                    "jitter_seconds": 5,
                    "lease_duration_seconds": 60,
                    "item_timeout_seconds": 30,
                    "low_balance_threshold": 100,
                    "health_refresh_interval_seconds": 60,
                    "evidence_stale_after_seconds": 300
                }
            },
            "mail": complete_mail()
        })
    }

    #[test]
    fn operations_accepts_the_complete_default_projection() {
        assert!(valid_operations(&default_operations()));
    }

    #[test]
    fn operations_rejects_missing_and_extra_projection_fields() {
        for (section, required, extra) in [
            ("", "worker", "extra"),
            ("/economics", "start_price", "extra"),
            ("/schedule", "open_time", "extra"),
            ("/forum", "maximum_topic_pages", "extra"),
            ("/alerts", "source_timeout_seconds", "extra"),
            ("/worker", "concurrency", "extra"),
            ("/worker/delivery", "batch_limit", "concurrency"),
            ("/worker/fulfillment", "concurrency", "extra"),
            ("/worker/recovery", "concurrency", "extra"),
            ("/mail", "send_classes", "extra"),
        ] {
            let mut missing = default_operations();
            let object = missing.pointer_mut(section).and_then(Value::as_object_mut).unwrap();
            assert!(object.remove(required).is_some());
            assert!(!valid_operations(&missing), "{section}/{required}");

            let mut unexpected = default_operations();
            let object = unexpected.pointer_mut(section).and_then(Value::as_object_mut).unwrap();
            assert!(object.insert(extra.to_owned(), json!(1)).is_none());
            assert!(!valid_operations(&unexpected), "{section}/{extra}");
        }
    }

    #[test]
    fn early_bird_accepts_the_optional_pre_season_fields_independently() {
        let defaults = json!({
            "enabled": false,
            "timezone": "UTC",
            "worlds": ["isekai"],
            "season_starts": ["05-01", "11-01"],
            "season_start_time": "09:00",
            "window_days": 7,
            "session_days": [1, 3, 5, 7],
            "open_time": "18:00",
            "preview_lead_minutes": 120,
            "live_minutes": 1440,
            "qualities": [],
            "pause_intake_before_season": true
        });
        assert!(valid_early_bird(&defaults));
        let mut explicit = defaults.clone();
        explicit["pre_season_session"] = json!(false);
        assert!(valid_early_bird(&explicit));
        explicit["pre_season_days_before"] = json!(4);
        assert!(valid_early_bird(&explicit));
        explicit.as_object_mut().unwrap().remove("pre_season_session");
        assert!(valid_early_bird(&explicit));

        for (key, invalid) in [
            ("pre_season_session", json!("false")),
            ("pre_season_days_before", json!(0)),
            ("pre_season_days_before", json!(32)),
            ("pre_season_days_before", Value::Null),
            ("extra", json!(1)),
        ] {
            let mut early_bird = defaults.clone();
            early_bird[key] = invalid;
            assert!(!valid_early_bird(&early_bird), "{key}");
        }
        explicit.as_object_mut().unwrap().remove("worlds");
        assert!(!valid_early_bird(&explicit));
    }

    /// 带 `include=fulfillment_collection_overdue_age_seconds` 时服务端给出的完整告警区块。
    fn complete_alerts() -> Value {
        json!({
            "managed": false,
            "enabled": false,
            "scan_interval_seconds": 60,
            "jitter_seconds": 5,
            "source_timeout_seconds": 5,
            "batch_window_seconds": 120,
            "cooldown_seconds": 1800,
            "claim_lease_seconds": 120,
            "retry_delay_seconds": 300,
            "max_signals_per_scan": 64,
            "max_batch_items": 20,
            "max_body_bytes": 4096,
            "delivery_pending_count_threshold": 1,
            "delivery_pending_age_seconds": 900,
            "delivery_manual_reconcile_count_threshold": 1,
            "fulfillment_backlog_count_threshold": 1,
            "fulfillment_backlog_age_seconds": 900,
            "fulfillment_collection_overdue_age_seconds": 259_200,
            "recovery_overdue_count_threshold": 1,
            "recovery_overdue_age_seconds": 900,
            "world_id": "main",
            "recipient": ""
        })
    }

    /// 本页请求扩展投影，告警区块必须带长期未领取阈值且在 1–2592000 内；缺省投影（已发布闭集，
    /// 没有该键）不是本页请求的形状，解码失败关闭，不自备默认值。
    #[test]
    fn alerts_block_requires_the_collection_threshold_projection() {
        assert!(valid_alerts(&complete_alerts()));
        let mut baseline = complete_alerts();
        baseline
            .as_object_mut()
            .expect("告警区块")
            .remove("fulfillment_collection_overdue_age_seconds");
        assert!(!valid_alerts(&baseline), "缺省投影的告警区块必须拒绝");
        for (seconds, valid) in [(0, false), (1, true), (2_592_000, true), (2_592_001, false)] {
            let mut alerts = complete_alerts();
            alerts["fulfillment_collection_overdue_age_seconds"] = json!(seconds);
            assert_eq!(valid_alerts(&alerts), valid, "{seconds}");
        }
    }

    /// 服务端对旧文档补齐后的完整邮件区块（全部为默认值）。
    fn complete_mail() -> Value {
        json!({
            "managed": false,
            "send_intervals": {
                "outbid_seconds": 0,
                "account_seconds": 0,
                "intake_seconds": 0,
                "settlement_seconds": 0,
                "equipment_seconds": 0,
                "operations_seconds": 0,
                "manual_seconds": 0
            },
            "schedule": "*/5 * * * *",
            "maximum_response_bytes": 8_388_608,
            "minimum_request_interval_millis": 1000,
            "request_timeout_seconds": 15,
            "batch_size": 100,
            "maximum_pages": 20,
            "jitter_seconds": 5,
            "lease_duration_seconds": 60,
            "renew_interval_seconds": 10,
            "fetch_attempts": 3,
            "fetch_timeout_seconds": 30,
            "process_attempts": 3,
            "item_timeout_seconds": 30,
            "batch_timeout_seconds": 600,
            "retry_base_seconds": 1,
            "retry_maximum_seconds": 30,
            "rekey_enabled": true,
            "account_reply_window_seconds": 86_400,
            "explanation_mail_window_seconds": 86_400,
            "explanation_mail_limit": 10,
            "send_classes": {
                "account_reply": true,
                "intake_reply": true,
                "settlement_notice": true,
                "outbid_reminder": true,
                "player_summary": true,
                "operations_alert": true
            }
        })
    }

    /// 浏览器不再自备默认值：缺少任一由服务端补齐的键都视为响应损坏。
    #[test]
    fn mail_block_requires_the_complete_server_projection() {
        assert!(valid_mail(&complete_mail()));
        for key in [
            "batch_timeout_seconds",
            "send_intervals",
            "send_classes",
            "rekey_enabled",
            "account_reply_window_seconds",
            "explanation_mail_window_seconds",
            "explanation_mail_limit",
        ] {
            let mut mail = complete_mail();
            mail.as_object_mut().expect("邮件区块").remove(key);
            assert!(!valid_mail(&mail), "缺少 {key} 必须拒绝");
        }
        let mut partial_classes = complete_mail();
        partial_classes["send_classes"].as_object_mut().expect("发送开关").remove("player_summary");
        assert!(!valid_mail(&partial_classes), "发送开关缺键必须拒绝");
        let mut unknown = complete_mail();
        unknown.as_object_mut().expect("邮件区块").insert("extra".to_owned(), json!(1));
        assert!(!valid_mail(&unknown), "未知键必须拒绝");
    }
}
