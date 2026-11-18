//! 运行参数响应的字段闭集、枚举及安全整数校验。

use auction_web_shared::wire::json_exact_object;
use serde_json::Value;

use super::fields::{safe_u64, valid_scalars, valid_scalars_with_signed, valid_string_array};

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

/// 服务端已补齐邮件区块的默认值。缺键即视为响应结构损坏。
fn valid_mail(value: &Value) -> bool {
    valid_scalars(
        value,
        &["managed", "rekey_enabled"],
        &["schedule"],
        &[
            "maximum_response_bytes",
            "minimum_request_interval_millis",
            "request_timeout_seconds",
            "batch_size",
            "maximum_pages",
            "jitter_seconds",
            "lease_duration_seconds",
            "renew_interval_seconds",
            "fetch_attempts",
            "fetch_timeout_seconds",
            "process_attempts",
            "item_timeout_seconds",
            "batch_timeout_seconds",
            "retry_base_seconds",
            "retry_maximum_seconds",
            "account_reply_window_seconds",
            "explanation_mail_window_seconds",
            "explanation_mail_limit",
        ],
        &["send_intervals", "send_classes"],
    ) && valid_send_intervals(&value["send_intervals"])
        && valid_send_classes(&value["send_classes"])
}

fn valid_send_intervals(value: &Value) -> bool {
    valid_scalars(
        value,
        &[],
        &[],
        &[
            "outbid_seconds",
            "account_seconds",
            "intake_seconds",
            "settlement_seconds",
            "equipment_seconds",
            "operations_seconds",
            "manual_seconds",
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
    if !valid_scalars_with_signed(
        value,
        &["managed", "proxy_enabled"],
        &["proxy_default_mode"],
        &["proxy_max_outbid_uses"],
        &[
            "start_price",
            "bid_increment_ppm",
            "soft_close_window_seconds",
            "soft_close_extension_seconds",
            "cancel_cutoff_seconds",
            "proxy_slow_interval_seconds",
            "final_phase_lead_seconds",
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
            "timezone",
            "preview_day",
            "preview_time",
            "open_day",
            "open_time",
            "nominal_end_day",
            "nominal_end_time",
        ],
        &[],
        nested,
    ) {
        return false;
    }
    valid_world_list(&value["worlds"])
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

fn valid_world_list(value: &Value) -> bool {
    value.as_array().is_some_and(|worlds| {
        worlds.iter().all(|world| matches!(world.as_str(), Some("main" | "isekai")))
    })
}

fn valid_early_bird(value: &Value) -> bool {
    let mut booleans = vec!["enabled", "pause_intake_before_season"];
    let mut numbers = vec!["window_days", "preview_lead_minutes", "live_minutes"];
    // 赛季前场次的两项默认值由后端各自省略。
    if value.get("pre_season_session").is_some() {
        booleans.push("pre_season_session");
    }
    if value.get("pre_season_days_before").is_some() {
        numbers.push("pre_season_days_before");
    }
    valid_scalars(
        value,
        &booleans,
        &["timezone", "season_start_time", "open_time"],
        &numbers,
        &["worlds", "season_starts", "session_days", "qualities"],
    ) && valid_world_list(&value["worlds"])
        && valid_string_array(&value["season_starts"])
        && value["session_days"]
            .as_array()
            .is_some_and(|days| days.iter().all(|day| safe_u64(day).is_some()))
        && valid_string_array(&value["qualities"])
}

fn valid_forum(value: &Value) -> bool {
    valid_scalars(
        value,
        &["managed", "enabled"],
        &[],
        &[
            "batch_size",
            "poll_interval_seconds",
            "jitter_seconds",
            "maximum_response_bytes",
            "maximum_topic_pages",
            "minimum_request_interval_millis",
            "request_timeout_seconds",
            "item_timeout_seconds",
            "lease_duration_seconds",
        ],
        &[],
    )
}

fn valid_alerts(value: &Value) -> bool {
    if !valid_scalars_with_signed(
        value,
        &["managed", "enabled"],
        &["world_id", "recipient"],
        &[
            "scan_interval_seconds",
            "jitter_seconds",
            "source_timeout_seconds",
            "batch_window_seconds",
            "cooldown_seconds",
            "claim_lease_seconds",
            "retry_delay_seconds",
            "max_signals_per_scan",
            "max_batch_items",
            "max_body_bytes",
            "delivery_pending_age_seconds",
            "fulfillment_backlog_age_seconds",
            "fulfillment_collection_overdue_age_seconds",
            "recovery_overdue_age_seconds",
        ],
        &[
            "delivery_pending_count_threshold",
            "delivery_manual_reconcile_count_threshold",
            "fulfillment_backlog_count_threshold",
            "recovery_overdue_count_threshold",
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
            "concurrency",
            "poll_interval_seconds",
            "batch_limit",
            "jitter_seconds",
            "item_timeout_seconds",
            "recovery_retry_delay_seconds",
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
            "batch_limit",
            "poll_interval_seconds",
            "jitter_seconds",
            "lease_duration_seconds",
            "renew_interval_seconds",
            "sending_grace_seconds",
            "request_timeout_seconds",
            "finalize_timeout_seconds",
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
            "batch_limit",
            "concurrency",
            "poll_interval_seconds",
            "jitter_seconds",
            "lease_duration_seconds",
            "item_timeout_seconds",
            "retry_delay_seconds",
        ],
        &[],
    )
}

fn valid_worker_recovery(value: &Value) -> bool {
    valid_scalars_with_signed(
        value,
        &[],
        &[],
        &[
            "batch_limit",
            "concurrency",
            "poll_interval_seconds",
            "jitter_seconds",
            "lease_duration_seconds",
            "item_timeout_seconds",
            "health_refresh_interval_seconds",
            "evidence_stale_after_seconds",
        ],
        &["low_balance_threshold"],
        &[],
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::tests::default_response;
    use super::{valid_alerts, valid_early_bird, valid_mail, valid_operations};

    fn default_operations() -> Value {
        default_response().document["operations"].clone()
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
        for safe_integer in [0, 4, 32] {
            explicit["pre_season_days_before"] = json!(safe_integer);
            assert!(valid_early_bird(&explicit));
        }

        for (key, invalid) in [
            ("pre_season_session", json!("false")),
            ("pre_season_days_before", json!("4")),
            ("pre_season_days_before", json!(9_007_199_254_740_992_u64)),
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

    /// 扩展投影必须带长期未领取阈值。这里只校验整数的类型和浏览器安全边界。
    #[test]
    fn alerts_block_requires_the_collection_threshold_projection() {
        assert!(valid_alerts(&default_operations()["alerts"].clone()));
        let mut baseline = default_operations()["alerts"].clone();
        baseline
            .as_object_mut()
            .expect("告警区块")
            .remove("fulfillment_collection_overdue_age_seconds");
        assert!(!valid_alerts(&baseline), "缺省投影的告警区块必须拒绝");
        for (seconds, valid) in [(0, true), (2_592_001, true), (9_007_199_254_740_992_u64, false)] {
            let mut alerts = default_operations()["alerts"].clone();
            alerts["fulfillment_collection_overdue_age_seconds"] = json!(seconds);
            assert_eq!(valid_alerts(&alerts), valid, "{seconds}");
        }
    }

    /// 浏览器不再自备默认值：缺少任一由服务端补齐的键都视为响应损坏。
    #[test]
    fn mail_block_requires_the_complete_server_projection() {
        assert!(valid_mail(&default_operations()["mail"].clone()));
        for key in [
            "batch_timeout_seconds",
            "send_intervals",
            "send_classes",
            "rekey_enabled",
            "account_reply_window_seconds",
            "explanation_mail_window_seconds",
            "explanation_mail_limit",
        ] {
            let mut mail = default_operations()["mail"].clone();
            mail.as_object_mut().expect("邮件区块").remove(key);
            assert!(!valid_mail(&mail), "缺少 {key} 必须拒绝");
        }
        let mut partial_classes = default_operations()["mail"].clone();
        partial_classes["send_classes"].as_object_mut().expect("发送开关").remove("player_summary");
        assert!(!valid_mail(&partial_classes), "发送开关缺键必须拒绝");
        let mut unknown = default_operations()["mail"].clone();
        unknown.as_object_mut().expect("邮件区块").insert("extra".to_owned(), json!(1));
        assert!(!valid_mail(&unknown), "未知键必须拒绝");
    }
}
