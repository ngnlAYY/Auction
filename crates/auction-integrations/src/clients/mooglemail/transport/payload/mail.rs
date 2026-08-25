//! 普通 MoogleMail delivery payload 的安全校验。

use super::super::*;

pub(crate) fn decode_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(MailDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if !matches!(
        claim.origin_kind.as_str(),
        "mail_intake" | "settlement_recovery_action" | "operations_alert"
    ) || claim.delivery_id.is_nil()
        || !valid_line(&claim.recipient, 256, false)
        || !valid_line(&claim.origin_id, 512, false)
    {
        return Err(invalid_payload());
    }
    let payload: MailDeliveryPayload =
        serde_json::from_value(claim.payload.clone()).map_err(|_| invalid_payload())?;
    if payload.schema_version != SCHEMA_VERSION
        || !valid_line(&payload.world, 16, false)
        || !valid_line(&payload.external_mail_id, 256, false)
        || !valid_opaque_id(&payload.external_mail_id)
        || payload.credits < 0
        || payload.cod < 0
        || payload.attachment_ids.len() > MAX_ATTACHMENT_IDS
        || payload.attachment_ids.iter().any(|id| !valid_decimal_id(id))
        || payload
            .attachment_ids
            .iter()
            .enumerate()
            .any(|(index, id)| payload.attachment_ids[..index].contains(id))
    {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    let expected_origin_id =
        format!("{}:{}:{}", payload.world, payload.external_mail_id, payload.operation);
    if claim.origin_kind == "mail_intake" && claim.origin_id != expected_origin_id {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "operations_alert"
        && (claim.origin_id
            != payload
                .external_mail_id
                .strip_prefix("operations-alert:")
                .map(|value| format!("operations_alert:{value}"))
                .unwrap_or_default()
            || !valid_notification_body(payload.body.as_deref()))
    {
        return Err(invalid_payload());
    }
    if claim.origin_kind != "operations_alert" && payload.body.is_some() {
        return Err(invalid_payload());
    }
    if claim.origin_kind == "settlement_recovery_action" {
        let recovery_id =
            claim.origin_id.strip_prefix("recovery:").and_then(|value| Uuid::parse_str(value).ok());
        if recovery_id.is_none()
            || (payload.operation == "send" && payload.world != "main")
            || !matches!(payload.operation.as_str(), "send" | "take")
        {
            return Err(invalid_payload());
        }
    }
    match payload.operation.as_str() {
        "send" => {
            if payload.template.is_none()
                || payload.result_code.is_none()
                || payload.credits != 0
                || payload.cod != 0
                || !payload.attachment_ids.is_empty()
            {
                return Err(invalid_payload());
            }
            match payload.template.as_deref() {
                Some("register_key") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_none()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("registered" | "recovered")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("register_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some("blacklisted" | "identity_conflict")
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("rebind_result") => {
                    if claim.origin_kind != "mail_intake"
                        || payload.user_key_user_id.is_some()
                        || !matches!(
                            payload.result_code.as_deref(),
                            Some(
                                "rebound"
                                    | "unchanged"
                                    | "blacklisted"
                                    | "identity_conflict"
                                    | "invalid_key"
                                    | "missing_key"
                            )
                        )
                    {
                        return Err(invalid_payload());
                    }
                }
                Some(
                    "auction_assets_rejected"
                    | "auction_no_attachments"
                    | "auction_equipment_rejected"
                    | "user_assets_rejected",
                ) => {
                    if claim.origin_kind != "mail_intake" || payload.user_key_user_id.is_some() {
                        return Err(invalid_payload());
                    }
                }
                Some("recovery_payment_reminder") => {
                    if claim.origin_kind != "settlement_recovery_action"
                        || payload.result_code.as_deref() != Some("payment_overdue")
                        || payload.user_key_user_id.is_some()
                    {
                        return Err(invalid_payload());
                    }
                }
                Some("operations_alert") => {
                    if claim.origin_kind != "operations_alert"
                        || payload.result_code.as_deref() != Some("operations_alert")
                        || payload.user_key_user_id.is_some()
                        || !valid_notification_body(payload.body.as_deref())
                    {
                        return Err(invalid_payload());
                    }
                }
                _ => return Err(invalid_payload()),
            }
        }
        "return" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.credits != 0
                || payload.cod != 0
                || payload.user_key_user_id.is_some()
                || claim.origin_kind != "mail_intake"
            {
                return Err(invalid_payload());
            }
        }
        "take" => {
            if payload.template.is_some()
                || payload.result_code.is_some()
                || payload.user_key_user_id.is_some()
                || (claim.origin_kind == "mail_intake"
                    && (payload.credits != 0 || payload.cod != 0))
            {
                return Err(invalid_payload());
            }
        }
        _ => return Err(invalid_payload()),
    }
    Ok((payload, world))
}
