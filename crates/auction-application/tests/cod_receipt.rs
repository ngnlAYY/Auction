//! COD 实收资金化的正反例；纯判定，不连接数据库。

use auction_application::{
    CodReceiptBlocker, CodReceiptResultMetadata, CodReceiptSnapshot, FundingCollectedReceipt,
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementInternalEvidenceSnapshot, VerifiedMailSentCollectionObservation,
    VerifiedMainCashObservation, canonical_settlement_fulfillment_subject,
};
use auction_domain::{
    LotId, SettlementAmount, SettlementId, SettlementLine, SettlementObligationKind,
    SettlementWorld,
};
use chrono::{DateTime, TimeZone, Utc};
use uuid::Uuid;

fn uuid(seed: u64) -> Uuid {
    Uuid::from_u64_pair(seed, seed)
}

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 2, 12, minute, 0)
        .single()
        .unwrap_or_else(|| panic!("固定测试时间必须存在"))
}

const BUYER: i64 = 1000;
const PAYMENT: u64 = 50;
const EQUIPMENT: u64 = 60;

fn line() -> SettlementLine {
    let amount =
        |value: i64| SettlementAmount::new(value).unwrap_or_else(|_| panic!("测试金额必须非负"));
    SettlementLine {
        lot_id: LotId::from_uuid(uuid(40)),
        end_price: amount(BUYER),
        listed_price: amount(BUYER),
        buyer_amount: amount(BUYER),
        seller_amount: amount(BUYER),
        tax_amount: amount(0),
        compensation_amount: amount(0),
        hold_ppm: 0,
        adjustment_ppm: 0,
    }
}

fn subject() -> String {
    canonical_settlement_fulfillment_subject(
        SettlementObligationKind::WinnerEquipmentCod,
        SettlementId::from_uuid(uuid(PAYMENT)),
    )
    .unwrap_or_else(|| panic!("COD 主题必须存在"))
}

fn payload() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_winner_equipment_cod",
        "variables": {
            "reference": uuid(PAYMENT).to_string(),
            "amount": BUYER.to_string(),
            "item_name": "Legendary Sword",
        },
        "credits": 0,
        "cod": BUYER,
        "equipment_secret_refs": [{
            "kind": "auction_equipment_key",
            "id": uuid(EQUIPMENT).to_string(),
        }],
    })
}

fn metadata() -> CodReceiptResultMetadata {
    CodReceiptResultMetadata {
        proof: Some("sent_exact_payload_v1".to_owned()),
        sent_verified: Some(true),
        settlement_obligation_id: Some(uuid(PAYMENT).to_string()),
        operation: Some("send".to_owned()),
        template: Some("settlement_winner_equipment_cod".to_owned()),
        external_reference_proven: Some(true),
    }
}

fn snapshot() -> CodReceiptSnapshot {
    CodReceiptSnapshot {
        obligation_kind: "winner_equipment_cod".to_owned(),
        obligation_amount: BUYER,
        carrier_credit: 0,
        equipment_id: uuid(EQUIPMENT),
        equipment_name: "Legendary Sword".to_owned(),
        payload: payload(),
        recipient: "Buyer".to_owned(),
        result_metadata: metadata(),
        settlement_line: Some(line()),
        settlement_mode: Some("hold_curve_v1".to_owned()),
        frozen_sent_proven: false,
        sending_at: Some(at(10)),
        funding_source_boundary: Some(at(0)),
    }
}

fn evaluate(snapshot: &CodReceiptSnapshot) -> Result<(), CodReceiptBlocker> {
    snapshot.evaluate_row(uuid(PAYMENT), "Buyer", &subject(), BUYER)
}

#[test]
fn exact_legacy_payload_is_admitted() {
    assert_eq!(evaluate(&snapshot()), Ok(()));
}

#[test]
fn funding_boundary_is_an_explicit_business_rejection() {
    let mut guard = snapshot();
    guard.sending_at = Some(at(0));
    guard.funding_source_boundary = Some(at(0));
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::SendBeforeFundingBoundary));

    let mut guard = snapshot();
    guard.sending_at = None;
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::SendingTimeMissing));

    let mut guard = snapshot();
    guard.funding_source_boundary = None;
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::OpeningSourceMissing));
}

#[test]
fn obligation_kind_carrier_and_amounts_fail_closed() {
    let mut guard = snapshot();
    guard.obligation_kind = "seller_payout".to_owned();
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::KindInvalid));

    let mut guard = snapshot();
    guard.carrier_credit = 1;
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::CarrierInvalid));

    let mut guard = snapshot();
    guard.obligation_amount = BUYER + 1;
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::AmountMismatch));

    let mut guard = snapshot();
    guard.obligation_amount = i64::MAX;
    guard.carrier_credit = 1;
    guard.obligation_kind = "winner_payment_carrier_cod".to_owned();
    assert_eq!(
        guard.evaluate_row(uuid(PAYMENT), "Buyer", &subject(), i64::MAX),
        Err(CodReceiptBlocker::CodOverflow)
    );
}

#[test]
fn recipient_subject_and_payload_must_match_the_frozen_facts() {
    let mut guard = snapshot();
    guard.recipient = "Another".to_owned();
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::RecipientMismatch));

    assert_eq!(
        snapshot().evaluate_row(uuid(PAYMENT), "Buyer", "wrong subject", BUYER),
        Err(CodReceiptBlocker::SubjectMismatch)
    );

    let mut guard = snapshot();
    guard.payload["credits"] = serde_json::json!(1);
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PayloadMismatch));

    let mut guard = snapshot();
    guard.payload["variables"]["item_name"] = serde_json::json!("Another Sword");
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PayloadMismatch));

    let mut guard = snapshot();
    guard.payload["variables"]["unknown"] = serde_json::json!("1");
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PayloadMismatch));

    let mut guard = snapshot();
    guard.payload["equipment_secret_refs"][0]["id"] = serde_json::json!(uuid(61).to_string());
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PayloadMismatch));
}

#[test]
fn proof_metadata_flags_are_mandatory() {
    let cases: Vec<(CodReceiptResultMetadata, CodReceiptBlocker)> = vec![
        (
            CodReceiptResultMetadata { operation: Some("return".to_owned()), ..metadata() },
            CodReceiptBlocker::OperationMismatch,
        ),
        (
            CodReceiptResultMetadata { template: Some("other".to_owned()), ..metadata() },
            CodReceiptBlocker::TemplateMismatch,
        ),
        (
            CodReceiptResultMetadata { external_reference_proven: None, ..metadata() },
            CodReceiptBlocker::ReferenceUnproven,
        ),
        (
            CodReceiptResultMetadata { sent_verified: None, ..metadata() },
            CodReceiptBlocker::SentProofUnproven,
        ),
        (
            CodReceiptResultMetadata {
                settlement_obligation_id: Some(uuid(PAYMENT + 1).to_string()),
                ..metadata()
            },
            CodReceiptBlocker::SentProofUnproven,
        ),
    ];
    for (metadata, expected) in cases {
        let mut guard = snapshot();
        guard.result_metadata = metadata;
        assert_eq!(evaluate(&guard), Err(expected));
    }

    let mut guard = snapshot();
    guard.result_metadata = CodReceiptResultMetadata::default();
    guard.frozen_sent_proven = true;
    // frozen 证明只替代 proof 标记，仍要求操作、模板与外部引用标记成立。
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::OperationMismatch));
}

#[test]
fn carrier_cod_payload_has_no_equipment_reference() {
    let buyer = BUYER;
    let cod = buyer + 1;
    let mut guard = snapshot();
    guard.obligation_kind = "winner_payment_carrier_cod".to_owned();
    guard.carrier_credit = 1;
    guard.obligation_amount = buyer;
    guard.equipment_name = "Unused".to_owned();
    guard.result_metadata = CodReceiptResultMetadata {
        template: Some("settlement_payment_carrier".to_owned()),
        ..metadata()
    };
    guard.payload = serde_json::json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_payment_carrier",
        "variables": {
            "reference": uuid(PAYMENT).to_string(),
            "amount": cod.to_string(),
        },
        "credits": 1,
        "cod": cod,
        "equipment_secret_refs": [],
    });
    let carrier_subject = canonical_settlement_fulfillment_subject(
        SettlementObligationKind::WinnerPaymentCarrierCod,
        SettlementId::from_uuid(uuid(PAYMENT)),
    )
    .unwrap_or_else(|| panic!("载体 COD 主题必须存在"));
    assert_eq!(guard.evaluate_row(uuid(PAYMENT), "Buyer", &carrier_subject, buyer), Ok(()));
}

#[test]
fn new_pricing_payload_requires_the_frozen_line() {
    let mut guard = snapshot();
    guard.payload = serde_json::json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_winner_equipment_cod",
        "variables": {
            "reference": uuid(PAYMENT).to_string(),
            "amount": BUYER.to_string(),
            "item_name": "Legendary Sword",
            "hold_ppm": "0",
            "adjustment_ppm": "0",
            "winning_bid": BUYER.to_string(),
            "adjusted_price": BUYER.to_string(),
            "pool_credit": "0",
        },
        "credits": 0,
        "cod": BUYER,
        "equipment_secret_refs": [{
            "kind": "auction_equipment_key",
            "id": uuid(EQUIPMENT).to_string(),
        }],
    });
    assert_eq!(evaluate(&guard), Ok(()));

    guard.settlement_line = None;
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PricingEndPriceMissing));

    guard.settlement_line = Some(line());
    guard.settlement_mode = Some("unknown_mode".to_owned());
    assert_eq!(evaluate(&guard), Err(CodReceiptBlocker::PricingModeInvalid));
}

fn binding() -> MailAccountEvidenceBinding {
    let mut binding = MailAccountEvidenceBinding {
        credential_id: uuid(1),
        credential_version: 1,
        runtime: MailRuntimeBinding { generation: 1 },
        authenticated_uid: "12345".to_owned(),
        player_name: "Bot".to_owned(),
        world: SettlementWorld::Main,
        observed_at: at(0),
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    binding.digest = binding.compute_digest();
    binding
}

fn sent(binding: &MailAccountEvidenceBinding) -> VerifiedMailSentCollectionObservation {
    let mut sent = VerifiedMailSentCollectionObservation {
        binding: binding.clone(),
        external_mail_id: "3120869".to_owned(),
        recipient: "Buyer".to_owned(),
        subject: subject(),
        sent_at: at(0),
        observed_at: at(1),
        remaining_credits: 0,
        cod_pending: 0,
        remaining_equipment_ids: Vec::new(),
        collected: true,
        digest: [0; 32],
    };
    sent.digest = sent.compute_digest();
    sent
}

fn cash(binding: &MailAccountEvidenceBinding) -> VerifiedMainCashObservation {
    let mut cash = VerifiedMainCashObservation {
        binding: binding.clone(),
        credits: 0,
        observed_at: at(0),
        digest: [0; 32],
    };
    cash.digest = cash.compute_digest();
    cash
}

fn evidence() -> SettlementInternalEvidenceSnapshot {
    let binding = binding();
    let mut evidence = SettlementInternalEvidenceSnapshot {
        operation_id: uuid(2),
        purpose: InternalEvidencePurpose::FundingReconciliation {
            command_id: uuid(3),
            expected_account_version: 0,
        },
        identity: binding.clone(),
        equipment: None,
        main_cash: Some(cash(&binding)),
        sent_collection: Some(sent(&binding)),
        observed_from: at(0),
        expires_at: at(1),
        digest: [0; 32],
    };
    evidence.digest = evidence.compute_digest();
    evidence
}

#[test]
fn evidence_binding_and_cash_mode_are_enforced() {
    let evidence = evidence();
    let receipt = |payment: Uuid, buyer: i64| FundingCollectedReceipt {
        payment_obligation_id: payment,
        delivery_id: uuid(70),
        buyer_credits: buyer,
        evidence: &evidence,
    };
    let guard = snapshot();
    assert_eq!(
        auction_application::evaluate_cod_receipt_evidence(
            &receipt(uuid(PAYMENT), BUYER),
            uuid(PAYMENT),
            BUYER,
            "12345",
            false,
        ),
        Ok(())
    );
    assert_eq!(
        guard.evaluate(&receipt(uuid(PAYMENT), BUYER), uuid(PAYMENT), BUYER, "12345", false),
        Ok(())
    );

    // record_only 不接受真实现金观察。
    assert_eq!(
        auction_application::evaluate_cod_receipt_evidence(
            &receipt(uuid(PAYMENT), BUYER),
            uuid(PAYMENT),
            BUYER,
            "12345",
            true,
        ),
        Err(CodReceiptBlocker::CashUnexpected)
    );

    assert_eq!(
        auction_application::evaluate_cod_receipt_evidence(
            &receipt(uuid(PAYMENT), BUYER),
            uuid(PAYMENT),
            BUYER,
            "99999",
            false,
        ),
        Err(CodReceiptBlocker::SentUidMismatch)
    );

    assert_eq!(
        auction_application::evaluate_cod_receipt_evidence(
            &receipt(uuid(PAYMENT + 1), BUYER),
            uuid(PAYMENT),
            BUYER,
            "12345",
            false,
        ),
        Err(CodReceiptBlocker::PaymentMismatch)
    );

    assert_eq!(
        auction_application::evaluate_cod_receipt_evidence(
            &receipt(uuid(PAYMENT), BUYER + 1),
            uuid(PAYMENT),
            BUYER,
            "12345",
            false,
        ),
        Err(CodReceiptBlocker::BuyerMismatch)
    );
}
