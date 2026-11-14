//! 旧版 MoogleMail schema-v1 结算 payload 校验与通知正文渲染。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use auction_application::settlement::canonical_settlement_fulfillment_subject;
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
        SettlementSendWirePayload::deserialize(&claim.payload).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "settlement_delivery",
                "decode_send_payload",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            invalid_payload()
        })?;
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
        SettlementReturnWirePayload::deserialize(&claim.payload).map_err(|diagnostic_cause| {
            safe_diagnostics::error(
                "settlement_delivery",
                "decode_return_payload",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            invalid_payload()
        })?;
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
    // 新变量只存在于新投递；升级前冻结的两/三变量载荷仍须可发送，用中性文案顶替缺项。
    let item_name = payload.variables.get("item_name").map_or(LEGACY_ITEM_NAME, String::as_str);
    let payment_deadline =
        payload.variables.get("payment_deadline").map_or(LEGACY_PAYMENT_DEADLINE, String::as_str);
    let body = match payload.template.as_str() {
        "settlement_winner_equipment_cod" => templates.render(
            "settlement_winner_equipment_cod",
            &[
                ("item_name", item_name),
                ("amount", amount.as_deref()?),
                ("reference", reference),
                ("price_breakdown", &pricing::render_price_breakdown(&payload.variables)?),
                ("payment_deadline", payment_deadline),
            ],
        )?,
        "settlement_payment_carrier" => templates.render(
            "settlement_payment_carrier",
            &[
                ("amount", amount.as_deref()?),
                ("reference", reference),
                ("item_name", item_name),
                ("payment_deadline", payment_deadline),
            ],
        )?,
        "settlement_equipment_handoff" => templates.render(
            "settlement_equipment_handoff",
            &[("item_name", payload.variables.get("item_name")?), ("reference", reference)],
        )?,
        "settlement_seller_payout" => templates.render(
            "settlement_seller_payout",
            &[("amount", amount.as_deref()?), ("reference", reference), ("item_name", item_name)],
        )?,
        "settlement_buyer_refund" => templates.render(
            "settlement_buyer_refund",
            &[("amount", amount.as_deref()?), ("reference", reference), ("item_name", item_name)],
        )?,
        _ => return None,
    };
    (valid_line(&subject, 512, false) && valid_body(&body)).then_some((subject, body))
}

/// 升级前冻结的两变量金额邮件没有装备名，正文用中性文案替代而不改写已存载荷。
const LEGACY_ITEM_NAME: &str = "your auction item";

/// 升级前冻结的 COD/载体载荷没有付款期限；正文明确指向原记录而不是编造日期。
const LEGACY_PAYMENT_DEADLINE: &str = "see your auction record";

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

/// 两变量旧金额载荷与该模板的新载荷；键集合必须精确命中其中之一。
const CREDIT_LEGACY_VARIABLES: &[&str] = &["amount", "reference"];
const CARRIER_VARIABLES: &[&str] = &["amount", "item_name", "payment_deadline", "reference"];
const CREDIT_VARIABLES: &[&str] = &["amount", "item_name", "reference"];

fn valid_settlement_variables(template: &str, variables: &BTreeMap<String, String>) -> bool {
    let reference = variables.get("reference").map(String::as_str);
    let valid_reference = reference.is_some_and(canonical_uuid);
    let valid_amount = || variables.get("amount").is_some_and(|value| valid_positive_amount(value));
    let valid_item_name =
        || variables.get("item_name").is_some_and(|value| valid_line(value, 512, false));
    let valid_optional_deadline =
        || variables.get("payment_deadline").is_none_or(|value| valid_payment_deadline(value));
    match template {
        "settlement_winner_equipment_cod" => {
            pricing::valid_winner_variables(variables)
                && valid_item_name()
                && valid_amount()
                && valid_optional_deadline()
                && valid_reference
        }
        "settlement_payment_carrier" => {
            valid_credit_variables(variables, CARRIER_VARIABLES)
                && valid_amount()
                && valid_reference
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            valid_credit_variables(variables, CREDIT_VARIABLES) && valid_amount() && valid_reference
        }
        "settlement_equipment_handoff" => {
            exact_variables(variables, &["item_name", "reference"])
                && variables.get("item_name").is_some_and(|value| valid_line(value, 512, false))
                && valid_reference
        }
        _ => false,
    }
}

/// 金额类邮件接受历史两变量载荷或带装备名（以及付款邮件的冻结期限）的新载荷。
fn valid_credit_variables(variables: &BTreeMap<String, String>, current: &[&str]) -> bool {
    let legacy = exact_variables(variables, CREDIT_LEGACY_VARIABLES);
    if legacy {
        return true;
    }
    exact_variables(variables, current)
        && variables.get("item_name").is_some_and(|value| valid_line(value, 512, false))
        && variables.get("payment_deadline").is_none_or(|value| valid_payment_deadline(value))
}

/// 正文期限只接受规范 UTC 秒精度文本，避免把本地时区或可读日期当成冻结事实。
fn valid_payment_deadline(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).ok().is_some_and(|deadline| {
        deadline.to_rfc3339_opts(chrono::SecondsFormat::Secs, true) == value
    })
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

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::clients::mooglemail::transport::payload::settlement as parent;

    const REFERENCE: &str = "0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11";
    const DEADLINE: &str = "2026-11-10T12:00:00Z";

    fn payload(
        template: &str,
        variables: &[(&str, &str)],
        credits: i64,
        cod: i64,
    ) -> SettlementDeliveryPayload {
        SettlementDeliveryPayload {
            operation: "send".to_owned(),
            template: template.to_owned(),
            variables: variables
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            credits,
            cod,
            equipment_secret_refs: Vec::new(),
        }
    }

    #[test]
    fn carrier_accepts_legacy_and_item_deadline_variables() {
        assert!(valid_settlement_variables(
            "settlement_payment_carrier",
            &BTreeMap::from([
                ("amount".to_owned(), "1001".to_owned()),
                ("reference".to_owned(), REFERENCE.to_owned()),
            ])
        ));
        assert!(valid_settlement_variables(
            "settlement_payment_carrier",
            &BTreeMap::from([
                ("amount".to_owned(), "1001".to_owned()),
                ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
                ("payment_deadline".to_owned(), DEADLINE.to_owned()),
                ("reference".to_owned(), REFERENCE.to_owned()),
            ])
        ));
        // 未登记变量与非法期限都不能被当作同一冻结形状。
        assert!(!valid_settlement_variables(
            "settlement_payment_carrier",
            &BTreeMap::from([
                ("amount".to_owned(), "1001".to_owned()),
                ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
                ("reference".to_owned(), REFERENCE.to_owned()),
                ("world".to_owned(), "main".to_owned()),
            ])
        ));
        assert!(!valid_settlement_variables(
            "settlement_payment_carrier",
            &BTreeMap::from([
                ("amount".to_owned(), "1001".to_owned()),
                ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
                ("payment_deadline".to_owned(), "2026-11-10".to_owned()),
                ("reference".to_owned(), REFERENCE.to_owned()),
            ])
        ));
    }

    #[test]
    fn credit_mails_accept_legacy_and_item_name_variables() {
        for template in ["settlement_seller_payout", "settlement_buyer_refund"] {
            assert!(valid_settlement_variables(
                template,
                &BTreeMap::from([
                    ("amount".to_owned(), "5250000".to_owned()),
                    ("reference".to_owned(), REFERENCE.to_owned()),
                ])
            ));
            assert!(valid_settlement_variables(
                template,
                &BTreeMap::from([
                    ("amount".to_owned(), "5250000".to_owned()),
                    ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
                    ("reference".to_owned(), REFERENCE.to_owned()),
                ])
            ));
            // 金额邮件不接受付款期限：期限只属于付款请求。
            assert!(!valid_settlement_variables(
                template,
                &BTreeMap::from([
                    ("amount".to_owned(), "5250000".to_owned()),
                    ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
                    ("payment_deadline".to_owned(), DEADLINE.to_owned()),
                    ("reference".to_owned(), REFERENCE.to_owned()),
                ])
            ));
        }
    }

    #[test]
    fn cod_accepts_optional_deadline_without_touching_price_set() {
        let legacy = BTreeMap::from([
            ("amount".to_owned(), "100".to_owned()),
            ("item_name".to_owned(), "Radiant Phase Shoes".to_owned()),
            ("reference".to_owned(), REFERENCE.to_owned()),
        ]);
        assert!(valid_settlement_variables("settlement_winner_equipment_cod", &legacy));
        let with_deadline = legacy
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .chain([("payment_deadline".to_owned(), DEADLINE.to_owned())])
            .collect();
        assert!(valid_settlement_variables("settlement_winner_equipment_cod", &with_deadline));
        let mut half_pricing = with_deadline;
        half_pricing.insert("hold_ppm".to_owned(), "500000".to_owned());
        // 半套价格变量必须继续被拒绝，期限不能掩盖缺字段。
        assert!(!valid_settlement_variables("settlement_winner_equipment_cod", &half_pricing));
    }

    #[test]
    fn rendered_payment_mail_states_item_and_deadline() {
        let templates = MoogleMailNotificationTemplates::default();
        let current = payload(
            "settlement_payment_carrier",
            &[
                ("amount", "1001"),
                ("item_name", "Radiant Phase Shoes"),
                ("payment_deadline", DEADLINE),
                ("reference", REFERENCE),
            ],
            1,
            1001,
        );
        let (subject, body) = render_settlement(&current, &templates).expect("新载荷必须可渲染");
        assert_eq!(subject, format!("Auction payment request [{REFERENCE}]"));
        assert!(body.contains("Item: Radiant Phase Shoes"), "{body}");
        assert!(body.contains(&format!("Payment deadline: {DEADLINE}")), "{body}");
        assert!(body.contains("CoD: 1,001 Credits"), "{body}");
    }

    #[test]
    fn legacy_frozen_payloads_still_render_with_neutral_copy() {
        let templates = MoogleMailNotificationTemplates::default();
        // 升级前冻结的两变量载荷没有装备名与期限，仍须可发送且不臆造事实。
        let carrier = payload(
            "settlement_payment_carrier",
            &[("amount", "1001"), ("reference", REFERENCE)],
            1,
            1001,
        );
        let (_, body) = render_settlement(&carrier, &templates).expect("旧载荷必须可渲染");
        assert!(body.contains("Item: your auction item"), "{body}");
        assert!(body.contains("Payment deadline: see your auction record"), "{body}");

        let payout = payload(
            "settlement_seller_payout",
            &[("amount", "5250000"), ("reference", REFERENCE)],
            5_250_000,
            0,
        );
        let (subject, body) = render_settlement(&payout, &templates).expect("旧载荷必须可渲染");
        assert_eq!(subject, format!("Auction seller payout [{REFERENCE}]"));
        assert!(body.contains("Item: your auction item"), "{body}");
        assert!(body.contains("Amount: 5,250,000 Credits"), "{body}");
    }

    #[test]
    fn rendered_credit_mail_states_item_and_amount() {
        let templates = MoogleMailNotificationTemplates::default();
        let refund = payload(
            "settlement_buyer_refund",
            &[
                ("amount", "5250000"),
                ("item_name", "Radiant Phase Shoes"),
                ("reference", REFERENCE),
            ],
            5_250_000,
            0,
        );
        let (_, body) = render_settlement(&refund, &templates).expect("新载荷必须可渲染");
        assert!(body.contains("Item: Radiant Phase Shoes"), "{body}");
        assert!(body.contains("Amount: 5,250,000 Credits"), "{body}");
    }
}
