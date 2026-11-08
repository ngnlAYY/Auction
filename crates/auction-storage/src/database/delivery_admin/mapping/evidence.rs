//! result metadata 证据校验：失败引用、准备拒绝、watch 失效、告警终止与隔离处置。

use auction_application::delivery::DeliveryAdminSnapshot;
use serde_json::Value;

pub(super) fn reason_code_from_metadata(metadata: &Value) -> Option<String> {
    let value = metadata.get("reason_code").and_then(Value::as_str)?;
    let trimmed = value.trim();
    (value == trimmed && !trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(super) fn take_required_from_metadata(metadata: &Value) -> bool {
    metadata.get("take_required").and_then(Value::as_bool).unwrap_or(false)
}

pub(super) fn returned_mail_id_from_metadata(metadata: &Value) -> Option<String> {
    let value = metadata.get("returned_mail_id").and_then(Value::as_str)?;
    if !valid_returned_mail_id(value) {
        return None;
    }
    Some(value.to_owned())
}

pub(super) fn valid_returned_mail_id(value: &str) -> bool {
    (1..=32).contains(&value.len()) && value.bytes().all(|character| character.is_ascii_digit())
}

/// 唯一可带引用的未知失败是已识别收件人不存在、仍待回收资产的退信；MID 不是 Sent 证明。
pub(super) fn valid_returned_reference(snapshot: &DeliveryAdminSnapshot) -> bool {
    snapshot.status == "manual_reconcile"
        && snapshot.failure_code.as_deref() == Some("recipient_not_found")
        && snapshot.reason_code.as_deref() == Some("recipient_not_found")
        && snapshot.cod.is_some()
        && snapshot.take_required
        && snapshot.returned_mail_id.as_deref().is_some_and(|mid| {
            valid_returned_mail_id(mid) && snapshot.external_reference.as_deref() == Some(mid)
        })
}

/// 列表及完整命令账本共用 transport 的精确形状，不能把任意失败引用降级成安全退信。
pub(super) fn valid_failed_reference_metadata(
    snapshot: &DeliveryAdminSnapshot,
    payload: &Value,
    metadata: &Value,
) -> bool {
    if snapshot.external_reference.is_none() || snapshot.status == "succeeded" {
        return true;
    }
    valid_returned_reference(snapshot)
        && matches!(payload.get("world").and_then(Value::as_str), Some("main" | "isekai"))
        && metadata.get("stage").and_then(Value::as_str) == Some("send")
        && metadata.get("operation").and_then(Value::as_str) == Some("send")
        && metadata.get("category").and_then(Value::as_str) == Some("recipient_not_found")
        && metadata.get("recipient_skipped_for_cycle").and_then(Value::as_bool) == Some(true)
        && metadata.get("retryable").and_then(Value::as_bool) == Some(false)
        && metadata.get("take_required").and_then(Value::as_bool) == Some(true)
        && metadata.get("returned_mail_id").and_then(Value::as_str)
            == snapshot.external_reference.as_deref()
        && metadata.get("outcome_unknown").is_none_or(|value| value == true)
        && metadata.get("external_reference_proven").is_none_or(|value| value == false)
        && metadata.get("sent_verified").is_none_or(|value| value == false)
        && metadata.get("proof").is_none()
}

pub(super) fn preparation_rejected(snapshot: &DeliveryAdminSnapshot) -> bool {
    snapshot.status == "failed"
        && matches!(snapshot.failure_code.as_deref(), Some("invalid_payload" | "task_panic"))
        && snapshot.sending_at.is_none()
}

/// 列表和完整命令账本都必须验证持久化的未写入标记；不能只凭无 sending 时间放宽终态。
pub(super) fn valid_preparation_rejection_metadata(
    snapshot: &DeliveryAdminSnapshot,
    metadata: &Value,
) -> bool {
    !preparation_rejected(snapshot)
        || (metadata.get("stage").and_then(Value::as_str) == Some("preflight_blocked")
            && metadata.get("failure_code").and_then(Value::as_str)
                == snapshot.failure_code.as_deref()
            && metadata.get("write_started").and_then(Value::as_bool) == Some(false)
            && metadata.get("outcome_unknown").and_then(Value::as_bool) == Some(false)
            && metadata.get("proof").is_none()
            && metadata.get("sent_verified").is_none_or(|value| value == false)
            && metadata.get("external_reference_proven").is_none_or(|value| value == false))
}

/// 围栏前退役的提醒（watch 失效/改名、换季提醒边界已过或义务完成）只允许携带确定未写入的标记。
pub(super) fn valid_preflight_retirement_metadata(metadata: &Value) -> bool {
    !matches!(
        metadata.get("reason_code").and_then(Value::as_str),
        Some(
            "lot_watch_invalidated"
                | "recipient_renamed"
                | "season_boundary_passed"
                | "season_reminder_resolved"
        )
    ) || (metadata.get("outcome_unknown").and_then(Value::as_bool) == Some(false)
        && metadata.get("preflight_blocked").and_then(Value::as_bool) == Some(true))
}

pub(super) fn valid_terminal_alert_metadata(metadata: &Value) -> bool {
    metadata.get("reason_code").and_then(Value::as_str) != Some("operations_alert_batch_terminal")
        || metadata.get("outcome_unknown").and_then(Value::as_bool) == Some(false)
}

pub(super) fn valid_quarantined_mail_metadata(metadata: &Value, status: &str) -> bool {
    if metadata.get("reason_code").and_then(Value::as_str) != Some("mail_disposition_conflict") {
        return true;
    }
    if metadata.get("preflight_blocked").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let outcome_unknown = metadata.get("outcome_unknown").and_then(Value::as_bool);
    let resolution = metadata.get("resolution").and_then(Value::as_str);
    match status {
        "manual_reconcile" => outcome_unknown == Some(true) && resolution.is_none(),
        "failed" => outcome_unknown == Some(false) && resolution == Some("admin_reconciled_failed"),
        _ => false,
    }
}
