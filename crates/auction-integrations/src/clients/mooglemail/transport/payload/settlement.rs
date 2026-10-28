//! 旧版 MoogleMail schema-v1 结算 payload 校验与通知正文渲染。

use auction_application::canonical_settlement_fulfillment_subject;
use auction_domain::{SettlementId, SettlementObligationKind};

use super::super::*;

mod pricing;

const MAXIMUM_PAYLOAD_BYTES: usize = 64 << 10;
const MAXIMUM_REFERENCES: usize = 100;

pub(crate) fn decode_settlement_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(SettlementDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    if claim.origin_kind != "settlement"
        || claim.delivery_id.is_nil()
        || !valid_line(&claim.recipient, 256, false)
        || !canonical_uuid(&claim.origin_id)
        || !valid_payload_size(&claim.payload)
    {
        return Err(invalid_payload());
    }
    let operation = claim
        .payload
        .as_object()
        .and_then(|object| object.get("operation"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid_payload)?;
    match operation {
        "send" => decode_send_payload(claim),
        "return" => decode_return_payload(claim),
        _ => Err(invalid_payload()),
    }
}

fn valid_payload_size(payload: &serde_json::Value) -> bool {
    // 按原紧凑 JSON 编码逐块扣减预算，不为大小校验复制整份载荷；超限立即停止。
    struct RemainingBytes(usize);

    impl std::io::Write for RemainingBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.checked_sub(bytes.len()).ok_or(std::io::ErrorKind::WriteZero)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(RemainingBytes(MAXIMUM_PAYLOAD_BYTES), payload).is_ok()
}

fn decode_send_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(SettlementDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    let mut payload =
        SettlementSendWirePayload::deserialize(&claim.payload).map_err(|_| invalid_payload())?;
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    if payload.schema_version != SCHEMA_VERSION
        || payload.operation != "send"
        || payload.credits < 0
        || payload.cod < 0
        || !valid_settlement_template(&payload.template)
        || !valid_settlement_variables(&payload.template, &payload.variables)
        || !canonical_secret_references(&mut payload.equipment_secret_refs)
        || !valid_send_semantics(&payload)
        || payload.variables.get("reference").map(String::as_str) != Some(claim.origin_id.as_str())
    {
        return Err(invalid_payload());
    }
    Ok((
        SettlementDeliveryPayload {
            operation: payload.operation,
            template: payload.template,
            variables: payload.variables,
            credits: payload.credits,
            cod: payload.cod,
            equipment_secret_refs: payload.equipment_secret_refs,
        },
        world,
    ))
}

fn decode_return_payload(
    claim: &DeliveryWorkClaim,
) -> Result<(SettlementDeliveryPayload, MoogleMailWorld), DeliveryTransportError> {
    let payload =
        SettlementReturnWirePayload::deserialize(&claim.payload).map_err(|_| invalid_payload())?;
    let world = MoogleMailWorld::parse(&payload.world).ok_or_else(invalid_payload)?;
    if payload.schema_version != SCHEMA_VERSION
        || payload.operation != "return"
        || !valid_line(&payload.external_mail_id, 256, false)
    {
        return Err(invalid_payload());
    }
    Ok((
        SettlementDeliveryPayload {
            operation: payload.operation,
            template: String::new(),
            variables: BTreeMap::new(),
            credits: 0,
            cod: 0,
            equipment_secret_refs: Vec::new(),
        },
        world,
    ))
}

pub(crate) fn render_settlement(
    payload: &SettlementDeliveryPayload,
    templates: &MoogleMailNotificationTemplates,
) -> Option<(String, String)> {
    let reference = payload.variables.get("reference")?;
    let obligation_id = Uuid::parse_str(reference).ok().map(SettlementId::from_uuid)?;
    let kind = match payload.template.as_str() {
        "settlement_winner_equipment_cod" => SettlementObligationKind::WinnerEquipmentCod,
        "settlement_payment_carrier" => SettlementObligationKind::WinnerPaymentCarrierCod,
        "settlement_equipment_handoff" => SettlementObligationKind::WinnerEquipmentHandoff,
        "settlement_seller_payout" => SettlementObligationKind::SellerPayout,
        "settlement_buyer_refund" => SettlementObligationKind::BuyerRefund,
        _ => return None,
    };
    let subject = canonical_settlement_fulfillment_subject(kind, obligation_id)?;
    let amount = payload
        .variables
        .get("amount")
        .and_then(|value| value.parse::<i64>().ok())
        .map(pricing::format_credits);
    let body = match payload.template.as_str() {
        "settlement_winner_equipment_cod" => templates.render(
            "settlement_winner_equipment_cod",
            &[
                ("item_name", payload.variables.get("item_name")?),
                ("amount", amount.as_deref()?),
                ("reference", reference),
                ("price_breakdown", &pricing::render_price_breakdown(&payload.variables)?),
            ],
        )?,
        "settlement_payment_carrier" => templates.render(
            "settlement_payment_carrier",
            &[("amount", amount.as_deref()?), ("reference", reference)],
        )?,
        "settlement_equipment_handoff" => templates.render(
            "settlement_equipment_handoff",
            &[("item_name", payload.variables.get("item_name")?), ("reference", reference)],
        )?,
        "settlement_seller_payout" => templates.render(
            "settlement_seller_payout",
            &[("amount", amount.as_deref()?), ("reference", reference)],
        )?,
        "settlement_buyer_refund" => templates.render(
            "settlement_buyer_refund",
            &[("amount", amount.as_deref()?), ("reference", reference)],
        )?,
        _ => return None,
    };
    (valid_line(&subject, 512, false) && valid_body(&body)).then_some((subject, body))
}

fn valid_settlement_template(value: &str) -> bool {
    matches!(
        value,
        "settlement_winner_equipment_cod"
            | "settlement_payment_carrier"
            | "settlement_equipment_handoff"
            | "settlement_seller_payout"
            | "settlement_buyer_refund"
    )
}

fn valid_settlement_variables(template: &str, variables: &BTreeMap<String, String>) -> bool {
    let reference = variables.get("reference").map(String::as_str);
    let valid_reference = reference.is_some_and(canonical_uuid);
    match template {
        "settlement_winner_equipment_cod" => {
            pricing::valid_winner_variables(variables)
                && variables.get("item_name").is_some_and(|value| valid_line(value, 512, false))
                && variables.get("amount").is_some_and(|value| valid_positive_amount(value))
                && valid_reference
        }
        "settlement_payment_carrier" | "settlement_seller_payout" | "settlement_buyer_refund" => {
            exact_variables(variables, &["amount", "reference"])
                && variables.get("amount").is_some_and(|value| valid_positive_amount(value))
                && valid_reference
        }
        "settlement_equipment_handoff" => {
            exact_variables(variables, &["item_name", "reference"])
                && variables.get("item_name").is_some_and(|value| valid_line(value, 512, false))
                && valid_reference
        }
        _ => false,
    }
}

fn exact_variables(variables: &BTreeMap<String, String>, names: &[&str]) -> bool {
    variables.len() == names.len() && names.iter().all(|name| variables.contains_key(*name))
}

fn valid_positive_amount(value: &str) -> bool {
    value.parse::<i64>().ok().is_some_and(|amount| amount > 0 && amount.to_string() == value)
}

fn valid_send_semantics(payload: &SettlementSendWirePayload) -> bool {
    let amount = payload.variables.get("amount").and_then(|value| value.parse::<i64>().ok());
    match payload.template.as_str() {
        "settlement_winner_equipment_cod" => {
            payload.credits == 0
                && payload.cod > 0
                && payload.equipment_secret_refs.len() == 1
                && amount == Some(payload.cod)
        }
        "settlement_payment_carrier" => {
            payload.credits > 0
                && payload.cod > payload.credits
                && payload.equipment_secret_refs.is_empty()
                && amount == Some(payload.cod)
        }
        "settlement_equipment_handoff" => {
            payload.credits == 0 && payload.cod == 0 && payload.equipment_secret_refs.len() == 1
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            payload.credits > 0
                && payload.cod == 0
                && payload.equipment_secret_refs.is_empty()
                && amount == Some(payload.credits)
        }
        _ => false,
    }
}

fn canonical_secret_references(values: &mut [SettlementEquipmentSecretReference]) -> bool {
    if values.len() > MAXIMUM_REFERENCES {
        return false;
    }
    values.sort_by(|left, right| left.kind.cmp(&right.kind).then_with(|| left.id.cmp(&right.id)));
    values.iter().enumerate().all(|(index, reference)| {
        reference.kind == "auction_equipment_key"
            && canonical_uuid(&reference.id)
            && (index == 0
                || values[index - 1].kind != reference.kind
                || values[index - 1].id != reference.id)
    })
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).ok().is_some_and(|id| !id.is_nil() && id.to_string() == value)
}
