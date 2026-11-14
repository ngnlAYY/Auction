//! 投递详情、预览与确认响应的严格解码。

use auction_contracts::{
    AdminDeliveryDetailResponse, AdminDeliverySentProofPreviewResponse,
    AdminDeliverySentProofResponse,
};
use uuid::Uuid;

use super::decode::{StrictDeliveryEnvelope, decode_delivery, parse_delivery_time};
use crate::api::{MAX_JSON_SAFE_INTEGER, valid_safe_text, valid_success_code, valid_uuid};

pub(super) fn decode_detail(
    payload: StrictDeliveryEnvelope<AdminDeliveryDetailResponse>,
) -> Result<AdminDeliveryDetailResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let data = payload.data;
    let _delivery = decode_delivery(data.delivery.clone())?;
    if data.attempts.iter().any(|attempt| !valid_attempt(attempt))
        || data.evidence.iter().any(|evidence| !valid_evidence(evidence))
        || !valid_custody(&data.custody)
        || data.dispositions.iter().any(|item| !valid_disposition(item))
        || data.obligation.as_ref().is_some_and(|item| !valid_obligation(item))
        || data.recovery.as_ref().is_some_and(|item| !valid_recovery(item))
    {
        return Err(());
    }
    Ok(data)
}

pub(super) fn decode_preview(
    payload: StrictDeliveryEnvelope<AdminDeliverySentProofPreviewResponse>,
    delivery_id: &str,
) -> Result<AdminDeliverySentProofPreviewResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let preview = payload.data;
    // 只接受本次请求的投递身份与正版本；blockers 是服务端闭集，不能由浏览器补全。
    if preview.delivery_id != delivery_id
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&preview.expected_fencing_version)
        || !valid_stored_text(&preview.external_reference)
        || preview.blockers.len() > 32
        || preview.blockers.iter().any(|blocker| !valid_safe_text(blocker, 96))
        || (preview.eligible && !preview.blockers.is_empty())
    {
        return Err(());
    }
    Ok(preview)
}

pub(super) fn decode_confirmation(
    payload: StrictDeliveryEnvelope<AdminDeliverySentProofResponse>,
    command_id: Uuid,
    delivery_id: &str,
) -> Result<AdminDeliverySentProofResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let result = payload.data;
    if result.command_id != command_id.to_string()
        || result.delivery_id != delivery_id
        || !valid_stored_text(&result.external_reference)
    {
        return Err(());
    }
    Ok(result)
}

pub(super) fn valid_attempt(value: &auction_contracts::AdminDeliveryAttemptResponse) -> bool {
    if !matches!(value.operation.as_str(), "send" | "take")
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.fencing_version)
        || !valid_stored_text(&value.world_id)
        || parse_delivery_time(&value.sending_at).is_none()
        || !value.recipient.as_deref().is_none_or(valid_stored_text)
        || !value.subject.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        || !value.body_digest.as_deref().is_none_or(valid_hex_digest)
        || !value.credits.is_none_or(valid_amount)
        || !value.cod.is_none_or(valid_amount)
        || !value.equipment_ids.iter().all(|id| valid_positive_decimal(id))
        || !value.external_mail_id.as_deref().is_none_or(valid_mail_id)
        || !value.bound_sent_mail_id.as_deref().is_none_or(valid_mail_id)
        || !value.credential.as_ref().is_none_or(|credential| {
            valid_uuid(&credential.credential_id)
                && (0..=MAX_JSON_SAFE_INTEGER).contains(&credential.credential_version)
                && (0..=MAX_JSON_SAFE_INTEGER).contains(&credential.runtime_generation)
        })
    {
        return false;
    }
    match value.operation.as_str() {
        "send" => {
            value.recipient.is_some()
                && value.subject.is_some()
                && value.credits.is_some()
                && value.cod.is_some()
        }
        _ => value.external_mail_id.is_some(),
    }
}

pub(super) fn valid_evidence(value: &auction_contracts::AdminDeliveryEvidenceResponse) -> bool {
    valid_safe_text(&value.reference, 128)
        && matches!(
            value.kind.as_str(),
            "mailbox_sweep"
                | "sent_reference_sighting"
                | "compose_discard"
                | "sent_operator_confirmed"
        )
        && value.external_mail_id.as_deref().is_none_or(valid_mail_id)
        && value.observed_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
        && value.sent_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
        && value.recipient.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        && value.subject.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        && value.body_digest.as_deref().is_none_or(valid_hex_digest)
        && value.cod_pending.is_none_or(valid_amount)
        && value.remaining_credits.is_none_or(valid_amount)
        && value.remaining_equipment_count.is_none_or(valid_amount)
        && value.receipt.as_deref().is_none_or(|text| text.len() <= 2048)
}

pub(super) fn valid_custody(value: &auction_contracts::AdminDeliveryCustodyResponse) -> bool {
    value.cod_pending.is_none_or(valid_amount)
        && value.remaining_credits.is_none_or(valid_amount)
        && value.receipt.as_deref().is_none_or(|text| text.len() <= 2048)
        && value.observed_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
}

pub(super) fn valid_disposition(
    value: &auction_contracts::AdminDeliveryDispositionResponse,
) -> bool {
    valid_uuid(&value.command_id)
        && matches!(
            value.kind.as_str(),
            "release"
                | "retry_failed"
                | "reconcile"
                | "sent_operator_confirmed"
                | "operator_acknowledged"
                | "auto_sent_proof"
                | "operator_sent_confirmation"
                | "admin_failed"
                | "equipment_loss"
                | "cod_returned"
                | "legacy_self"
                | "preflight_retired"
        )
        && parse_delivery_time(&value.occurred_at).is_some()
        && value.status.as_deref().is_none_or(valid_stored_text)
        && value
            .fencing_version
            .is_none_or(|version| (0..=MAX_JSON_SAFE_INTEGER).contains(&version))
        && value.failure_code.as_deref().is_none_or(valid_stored_text)
        && value.external_reference.as_deref().is_none_or(valid_stored_text)
        && value.actor.as_deref().is_none_or(|text| valid_safe_text(text, 128))
        && value.reason.as_deref().is_none_or(|text| text.len() <= 2048)
}

pub(super) fn valid_obligation(value: &auction_contracts::AdminDeliveryObligationResponse) -> bool {
    valid_uuid(&value.obligation_id)
        && valid_uuid(&value.auction_id)
        && valid_uuid(&value.lot_id)
        && valid_stored_text(&value.world_id)
        && valid_stored_text(&value.obligation_kind)
        && valid_stored_text(&value.business_state)
        && valid_amount(value.amount)
        && value.fulfillment_state.as_deref().is_none_or(valid_stored_text)
        && value.fulfillment_error_code.as_deref().is_none_or(valid_stored_text)
}

pub(super) fn valid_recovery(value: &auction_contracts::AdminDeliveryRecoveryResponse) -> bool {
    valid_uuid(&value.recovery_id)
        && valid_uuid(&value.payment_obligation_id)
        && valid_stored_text(&value.stage)
        && parse_delivery_time(&value.payment_deadline_at).is_some()
        && value.resolution.as_deref().is_none_or(valid_stored_text)
        && value.blocking_code.as_deref().is_none_or(valid_stored_text)
        && value.manual_reconcile_delivery_id.as_deref().is_none_or(valid_uuid)
}

pub(super) fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

pub(super) fn valid_amount(value: i64) -> bool {
    (0..=MAX_JSON_SAFE_INTEGER).contains(&value)
}

pub(super) fn valid_positive_decimal(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && !value.starts_with('0')
}

pub(super) fn valid_mail_id(value: &str) -> bool {
    (1..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
