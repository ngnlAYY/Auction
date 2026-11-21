//! 校验持久化结算退件事件的闭集细节与字段格式。

use auction_contracts::{
    ADMIN_SETTLEMENT_RETURN_BLOCKERS, ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
    ADMIN_SETTLEMENT_RETURN_PLANS, ADMIN_SETTLEMENT_RETURN_REASONS, AdminSettlementReturnEventKind,
};
use serde_json::{Map, Value};
use uuid::Uuid;

pub(super) fn valid_event_details(kind: AdminSettlementReturnEventKind, details: &Value) -> bool {
    use AdminSettlementReturnEventKind as Kind;
    match kind {
        Kind::Opened => {
            // 000114 补建案件时追加的首条事件。
            exact_object(details, &["backfilled"])
                .is_some_and(|value| value.get("backfilled") == Some(&Value::Bool(true)))
                || exact_object(details, &["match_state", "reason_code", "dispatch_no"])
                    .is_some_and(|value| {
                        code(value, "match_state", &ADMIN_SETTLEMENT_RETURN_MATCH_STATES)
                            && code(value, "reason_code", &ADMIN_SETTLEMENT_RETURN_REASONS)
                            && nullable_positive_integer(value, "dispatch_no")
                    })
        }
        Kind::Acknowledged | Kind::RecheckRequested | Kind::Held => {
            exact_object(details, &[]).is_some()
        }
        Kind::InboxClosed => exact_object(details, &["asset_location"])
            .is_some_and(|value| string(value, "asset_location") == Some("returned_in_inbox")),
        Kind::TakeRequested => {
            exact_object(details, &["plan"])
                .is_some_and(|value| code(value, "plan", &ADMIN_SETTLEMENT_RETURN_PLANS))
                || exact_object(details, &["handover"])
                    .is_some_and(|value| value.get("handover") == Some(&Value::Bool(true)))
        }
        Kind::TakeRetryRequested => {
            exact_object(details, &["delivery_id", "fencing_version", "observed_case_version"])
                .is_some_and(|value| {
                    uuid_field(value, "delivery_id")
                        && positive_integer(value, "fencing_version")
                        && positive_integer(value, "observed_case_version")
                })
        }
        Kind::ResendAuthorized => exact_object(details, &["dispatch_no"])
            .is_some_and(|value| positive_integer(value, "dispatch_no")),
        Kind::RecheckRebound => {
            // 000123 修复历史退件绑定时记录的来源。
            exact_object(details, &["migration", "backfill_orphan_ordering_repaired"]).is_some_and(
                |value| {
                    value.get("migration").and_then(Value::as_i64) == Some(123)
                        && value.get("backfill_orphan_ordering_repaired")
                            == Some(&Value::Bool(true))
                },
            ) || exact_object(
                details,
                &["match_state", "reason_code", "delivery_id", "dispatch_no", "verified_version"],
            )
            .is_some_and(|value| {
                code(value, "match_state", &ADMIN_SETTLEMENT_RETURN_MATCH_STATES)
                    && code(value, "reason_code", &ADMIN_SETTLEMENT_RETURN_REASONS)
                    && uuid_field(value, "delivery_id")
                    && positive_integer(value, "dispatch_no")
                    && positive_integer(value, "verified_version")
            })
        }
        Kind::RecheckUnchanged => {
            exact_object(details, &["reason_code"])
                .is_some_and(|value| string(value, "reason_code") == Some("mail_not_found"))
                || exact_object(details, &["match_state", "reason_code"]).is_some_and(|value| {
                    code(value, "match_state", &ADMIN_SETTLEMENT_RETURN_MATCH_STATES)
                        && code(value, "reason_code", &ADMIN_SETTLEMENT_RETURN_REASONS)
                })
        }
        Kind::TakeActivated => {
            exact_object(details, &["delivery_id"])
                .is_some_and(|value| uuid_field(value, "delivery_id"))
                || exact_object(details, &["delivery_id", "requeued", "reason", "restarts"])
                    .is_some_and(|value| {
                        uuid_field(value, "delivery_id")
                            && value.get("requeued") == Some(&Value::Bool(true))
                            && safe_code(value, "reason")
                            && positive_integer(value, "restarts")
                    })
                || exact_object(details, &["delivery_id", "requeued", "reason", "sweep_retries"])
                    .is_some_and(|value| {
                        uuid_field(value, "delivery_id")
                            && value.get("requeued") == Some(&Value::Bool(true))
                            && safe_code(value, "reason")
                            && integer_in_range(value, "sweep_retries", 0, 8)
                    })
        }
        Kind::TakeUnknown => {
            exact_object(details, &["failure_code", "delivery_id"]).is_some_and(|value| {
                matches!(
                    string(value, "failure_code"),
                    Some(
                        "settlement_return_take_conflict"
                            | "settlement_return_take_unknown"
                            | "settlement_return_take_not_started"
                            | "settlement_return_take_failed"
                            | "mail_disposition_conflict"
                    )
                ) && nullable_uuid_field(value, "delivery_id")
            })
        }
        Kind::TakeObserved => exact_object(
            details,
            &["observation", "delivery_id", "fencing_version", "sweep_retries"],
        )
        .is_some_and(|value| {
            matches!(
                string(value, "observation"),
                Some(
                    "assets_claimable"
                        | "assets_unclaimable"
                        | "assets_changed"
                        | "mail_missing"
                        | "receipt_unproven"
                )
            ) && uuid_field(value, "delivery_id")
                && positive_integer(value, "fencing_version")
                && integer_in_range(value, "sweep_retries", 0, 8)
        }),
        Kind::TakeProven => {
            exact_object(details, &["next", "dispatch_no", "proof"]).is_some_and(|value| {
                string(value, "next") == Some("resend")
                    && positive_integer(value, "dispatch_no")
                    && take_proof(value)
            }) || exact_object(details, &["next", "resend_blocked", "proof"]).is_some_and(|value| {
                string(value, "next") == Some("hold")
                    && nullable_blocker(value, "resend_blocked")
                    && take_proof(value)
            })
        }
        Kind::Resent => {
            exact_object(details, &["dispatch_no", "delivery_id"]).is_some_and(|value| {
                positive_integer(value, "dispatch_no") && uuid_field(value, "delivery_id")
            })
        }
    }
}

fn exact_object<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    let object = value.as_object()?;
    (object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key)))
        .then_some(object)
}

fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn code(object: &Map<String, Value>, key: &str, allowed: &[&str]) -> bool {
    string(object, key).is_some_and(|value| allowed.contains(&value))
}

fn safe_code(object: &Map<String, Value>, key: &str) -> bool {
    string(object, key).is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    })
}

fn positive_integer(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).and_then(Value::as_i64).is_some_and(|value| value > 0)
}

fn nullable_positive_integer(object: &Map<String, Value>, key: &str) -> bool {
    object
        .get(key)
        .is_some_and(|value| value.is_null() || value.as_i64().is_some_and(|value| value > 0))
}

fn integer_in_range(object: &Map<String, Value>, key: &str, minimum: i64, maximum: i64) -> bool {
    object
        .get(key)
        .and_then(Value::as_i64)
        .is_some_and(|value| (minimum..=maximum).contains(&value))
}

fn uuid_field(object: &Map<String, Value>, key: &str) -> bool {
    string(object, key).is_some_and(valid_uuid_string)
}

fn nullable_uuid_field(object: &Map<String, Value>, key: &str) -> bool {
    object
        .get(key)
        .is_some_and(|value| value.is_null() || value.as_str().is_some_and(valid_uuid_string))
}

fn valid_uuid_string(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| !uuid.is_nil() && uuid.to_string() == value)
}

fn nullable_blocker(object: &Map<String, Value>, key: &str) -> bool {
    object.get(key).is_some_and(|value| {
        value.is_null()
            || value.as_str().is_some_and(|value| ADMIN_SETTLEMENT_RETURN_BLOCKERS.contains(&value))
    })
}

fn take_proof(object: &Map<String, Value>) -> bool {
    matches!(
        string(object, "proof"),
        Some("action_removed_or_success_page" | "action_recovered_inbox_v1")
    )
}
