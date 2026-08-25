//! 结算 MoogleMail payload 校验与通知正文渲染。

use super::super::*;

pub(crate) fn decode_settlement_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(SettlementDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if claim.delivery_id.is_nil() || !valid_line(&claim.recipient, 256, false) {
        return Err(invalid_payload());
    }
    let payload: SettlementDeliveryPayload =
        serde_json::from_value(claim.payload.clone()).map_err(|_| invalid_payload())?;
    if payload.schema_version != 2
        || payload.obligation_id.is_nil()
        || payload.auction_id.is_nil()
        || payload.lot_id.is_nil()
        || payload.origin_command_id.is_nil()
        || payload.equipment_id.is_nil()
        || payload.submission_id.is_some_and(|submission_id| submission_id.is_nil())
        || payload.recipient != claim.recipient
        || claim.origin_id != payload.obligation_id.to_string()
        || !valid_line(&payload.world, 16, false)
        || !valid_line(&payload.template, 128, false)
        || !valid_line(&payload.item_name, 512, false)
        || !valid_decimal_id(&payload.external_equipment_id)
        || payload.amount < 0
        || payload.credits < 0
        || payload.cod < 0
    {
        return Err(invalid_payload());
    }
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    match payload.operation.as_str() {
        "send" => {
            if payload.submission_source_kind.is_some()
                && !matches!(payload.submission_source_kind.as_deref(), Some("mail" | "admin"))
                || payload.source_external_mail_id.is_some()
            {
                return Err(invalid_payload());
            }
            match payload.kind.as_str() {
                "winner_equipment_cod"
                    if payload.template == "settlement_winner_equipment_cod"
                        && world == MoogleMailWorld::Main
                        && payload.credits == 0
                        && payload.cod == payload.amount
                        && payload.amount > 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                "winner_payment_carrier_cod"
                    if payload.template == "settlement_payment_carrier"
                        && world == MoogleMailWorld::Isekai
                        && payload.carrier_credit > 0
                        && payload.credits == i64::from(payload.carrier_credit)
                        && payload.amount > 0
                        && payload.item_name.is_empty() => {}
                "winner_equipment_handoff"
                    if payload.template == "settlement_equipment_handoff"
                        && payload.credits == 0
                        && payload.cod == 0
                        && payload.amount == 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                "seller_payout"
                    if payload.template == "settlement_seller_payout"
                        && payload.credits == payload.amount
                        && payload.credits > 0
                        && payload.cod == 0
                        && payload.carrier_credit == 0
                        && payload.item_name.is_empty() => {}
                "buyer_refund"
                    if payload.template == "settlement_buyer_refund"
                        && payload.credits == payload.amount
                        && payload.credits > 0
                        && payload.cod == 0
                        && payload.carrier_credit == 0
                        && payload.item_name.is_empty() => {}
                "unsold_return" | "withdrawn_return"
                    if payload.template == "settlement_equipment_handoff"
                        && payload.credits == 0
                        && payload.cod == 0
                        && payload.amount == 0
                        && payload.carrier_credit == 0
                        && !payload.item_name.is_empty() => {}
                _ => return Err(invalid_payload()),
            }
            if payload.kind == "winner_payment_carrier_cod"
                && payload.cod
                    != payload.amount.checked_add(payload.credits).ok_or_else(invalid_payload)?
            {
                return Err(invalid_payload());
            }
        }
        "return" => {
            if !matches!(payload.kind.as_str(), "unsold_return" | "withdrawn_return")
                || !payload.template.is_empty()
                || !payload.item_name.is_empty()
                || payload.credits != 0
                || payload.cod != 0
                || payload.amount != 0
                || payload.carrier_credit != 0
                || payload.submission_source_kind.as_deref() != Some("mail")
                || payload
                    .source_external_mail_id
                    .as_deref()
                    .is_none_or(|value| !valid_opaque_id(value))
            {
                return Err(invalid_payload());
            }
        }
        _ => return Err(invalid_payload()),
    }
    Ok((payload, world))
}

pub(crate) fn render_settlement(
    payload: &SettlementDeliveryPayload,
    templates: &MoogleMailNotificationTemplates,
) -> Option<(String, String)> {
    let reference = payload.obligation_id.to_string();
    let amount = match payload.template.as_str() {
        "settlement_winner_equipment_cod" | "settlement_payment_carrier" => payload.cod,
        "settlement_seller_payout" | "settlement_buyer_refund" => payload.credits,
        "settlement_equipment_handoff" => 0,
        _ => return None,
    }
    .to_string();
    let (subject, body) = match payload.template.as_str() {
        "settlement_winner_equipment_cod" => (
            format!("Auction equipment delivery [{reference}]"),
            templates.render(
                "settlement_winner_equipment_cod",
                &[
                    ("item_name", &payload.item_name),
                    ("amount", &amount),
                    ("reference", &reference),
                ],
            )?,
        ),
        "settlement_payment_carrier" => (
            format!("Auction payment request [{reference}]"),
            templates.render(
                "settlement_payment_carrier",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "settlement_equipment_handoff" => (
            format!("Auction equipment delivery [{reference}]"),
            templates.render(
                "settlement_equipment_handoff",
                &[("item_name", &payload.item_name), ("reference", &reference)],
            )?,
        ),
        "settlement_seller_payout" => (
            "Auction seller payout".to_owned(),
            templates.render(
                "settlement_seller_payout",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "settlement_buyer_refund" => (
            "Auction buyer refund".to_owned(),
            templates.render(
                "settlement_buyer_refund",
                &[("amount", &amount), ("reference", &reference)],
            )?,
        ),
        "" => return None,
        _ => return None,
    };
    (valid_line(&subject, 512, false) && valid_line(&body, 64 * 1024, true))
        .then_some((subject, body))
}
