//! 人工 Sent 准入的两种准入与阻断项回归；只覆盖纯判定，不连接数据库或外部邮件。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, ManualSentEvidence, SentAdminFailure,
    SentAdmissionBlocker, SentAdmissionSnapshot, SentCredential, SentDelivery, SentEquipment,
    SentFulfillment, SentFundingAccount, SentFundingAllocation, SentFundingLot, SentLot,
    SentObligation, SentPayload, SentPayloadVariables, SentResultMetadata, SentSettlement,
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

const CREDITS: i64 = 300_000;
const DIGEST: [u8; 32] = [7; 32];
const EQUIPMENT_NAME: &str = "Legendary Ethereal Shortsword of Slaughter";

fn delivery() -> SentDelivery {
    SentDelivery {
        id: uuid(30),
        status: "manual_reconcile".to_owned(),
        failure_code: Some("write_unknown".to_owned()),
        fencing_version: 4,
        origin_kind: "settlement".to_owned(),
        external_reference: None,
        lease_token: None,
        lease_expires_at: None,
        recipient: "Buyer001".to_owned(),
        command_id: uuid(31),
        request_digest: Some("\\x01".to_owned()),
        activation_command_id: Some(uuid(32)),
        activation_request_digest: Some("\\x02".to_owned()),
        sending_at: Some(at(0)),
        completed_at: Some(at(2)),
        activated_at: Some(at(0)),
        result_metadata: SentResultMetadata {
            write_started: Some(true),
            retired: None,
            resolution: None,
        },
        payload: Some(SentPayload {
            schema_version: 1,
            operation: "send".to_owned(),
            world: "main".to_owned(),
            template: "settlement_seller_payout".to_owned(),
            variables: SentPayloadVariables {
                reference: Some(uuid(1)),
                amount: Some(CREDITS.to_string()),
                item_name: None,
            },
            credits: CREDITS,
            cod: 0,
            equipment_secret_refs: Vec::new(),
        }),
    }
}

fn obligation() -> SentObligation {
    SentObligation {
        id: uuid(1),
        lot_id: uuid(20),
        auction_id: uuid(21),
        equipment_id: uuid(22),
        world_id: "main".to_owned(),
        obligation_kind: "seller_payout".to_owned(),
        business_state: "ready".to_owned(),
        amount: CREDITS,
        carrier_credit: 0,
        recipient_user_id: None,
        recipient_game_identity: Some("Buyer001".to_owned()),
        prerequisite_obligation_id: Some(uuid(2)),
    }
}

fn fulfillment() -> SentFulfillment {
    SentFulfillment {
        execution_state: "manual_reconcile".to_owned(),
        last_error_code: Some("write_unknown".to_owned()),
        delivery_id: Some(uuid(30)),
        delivery_operation: "send".to_owned(),
        dispatch_world: "main".to_owned(),
        claim_token: None,
        claim_expires_at: None,
        proof_kind: None,
        proof_reference: None,
        proof_digest: None,
        proof_observed_at: None,
        prepare_command_id: Some(uuid(31)),
        prepare_request_digest: Some("\\x01".to_owned()),
        activate_command_id: Some(uuid(32)),
        activate_request_digest: Some("\\x02".to_owned()),
    }
}

fn expectation() -> DeliverySendExpectation {
    DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: "Buyer001".to_owned(),
        subject: "Auction settlement".to_owned(),
        body_digest: DIGEST,
        credits: CREDITS,
        cod: 0,
        equipment_ids: Vec::new(),
        credential: Some(DeliverySendCredentialBinding {
            credential_id: uuid(40),
            credential_version: 3,
            runtime_generation: 1,
        }),
    }
}

fn evidence() -> ManualSentEvidence {
    ManualSentEvidence {
        schema_version: 1,
        world_id: "main".to_owned(),
        external_mail_id: "3120869".to_owned(),
        sender_uid: "9001".to_owned(),
        sender_name: "Auction Bot".to_owned(),
        recipient: "Buyer001".to_owned(),
        subject: "Auction settlement".to_owned(),
        sent_minute: at(1),
        observed_at: at(6),
        original_body_digest: DIGEST,
        observed_body_digest: DIGEST,
        remaining_credits: CREDITS,
        cod_pending: 0,
        equipment_ids: Vec::new(),
        collected: false,
        receipt: String::new(),
    }
}

fn credential_fixture(snapshot: &SentAdmissionSnapshot) -> SentCredential {
    snapshot.credential.clone().unwrap_or_else(|| panic!("凭据夹具必须存在"))
}

fn snapshot() -> SentAdmissionSnapshot {
    SentAdmissionSnapshot {
        delivery: delivery(),
        obligation: Some(obligation()),
        fulfillment: Some(fulfillment()),
        recovery: None,
        admin_failure: None,
        funding_allocation: Some(SentFundingAllocation {
            id: uuid(50),
            uid: "9001".to_owned(),
            lot_id: uuid(20),
            obligation_id: uuid(1),
            delivery_id: uuid(30),
            purpose: "seller".to_owned(),
            state: "unknown".to_owned(),
            amount: CREDITS,
        }),
        funding_lot: Some(SentFundingLot {
            lot_id: uuid(20),
            uid: "9001".to_owned(),
            source_kind: "collected".to_owned(),
            disposition: "seller".to_owned(),
            payment_obligation_id: Some(uuid(2)),
        }),
        lot: Some(SentLot { auction_id: uuid(21), equipment_id: uuid(22) }),
        equipment: Some(SentEquipment {
            id: uuid(22),
            world_id: "main".to_owned(),
            external_equipment_id: "300753957".to_owned(),
            name: EQUIPMENT_NAME.to_owned(),
        }),
        settlement: Some(SentSettlement { buyer_amount: CREDITS, seller_amount: CREDITS }),
        credential: Some(SentCredential {
            id: Some(uuid(40)),
            kind: Some("mail_cookie".to_owned()),
            status: Some("active".to_owned()),
            scope: Some("mooglemail:main".to_owned()),
            version: Some(3),
        }),
        accounts: vec![SentFundingAccount { uid: "9001".to_owned(), status: "ready".to_owned() }],
        player_name: Some("Auction Bot".to_owned()),
        cash_reservation: true,
        prior_unbound: false,
        conflict: false,
        sending_fencing_version: 4,
        expectation: Some(expectation()),
    }
}

#[test]
fn write_unknown_seller_payout_admits() {
    let admission = snapshot().evaluate(&evidence(), at(7));
    assert_eq!(admission.blockers, Vec::new());
    assert!(admission.eligible);
    assert_eq!(admission.expected_fencing_version, 4);
    assert!(!admission.requires_attempt_history_acceptance);
    assert!(!admission.corrects_admin_failure);
}

#[test]
fn admin_failure_correction_admits_with_attempt_history_acceptance() {
    let mut snapshot = snapshot();
    snapshot.delivery.status = "failed".to_owned();
    snapshot.delivery.failure_code = Some("not_observed".to_owned());
    snapshot.delivery.result_metadata = SentResultMetadata {
        write_started: None,
        retired: None,
        resolution: Some("admin_reconciled_failed".to_owned()),
    };
    snapshot.delivery.sending_at = Some(at(3));
    snapshot.delivery.completed_at = Some(at(5));
    snapshot.delivery.activated_at = Some(at(0));
    snapshot.fulfillment = Some(SentFulfillment {
        execution_state: "failed".to_owned(),
        last_error_code: Some("not_observed".to_owned()),
        ..fulfillment()
    });
    snapshot.admin_failure = Some(SentAdminFailure {
        command_id: uuid(60),
        command_kind: "reconcile".to_owned(),
        status: Some("failed".to_owned()),
        fencing_version: Some(4),
        failure_code: Some("not_observed".to_owned()),
    });
    let mut evidence = evidence();
    evidence.sent_minute = at(2);
    evidence.observed_at = at(6);
    let admission = snapshot.evaluate(&evidence, at(7));
    assert_eq!(admission.blockers, Vec::new());
    assert!(admission.corrects_admin_failure);
    // 人工证据早于当前尝试：必须显式接受旧尝试关联。
    assert!(admission.requires_attempt_history_acceptance);
}

#[test]
fn claimed_delivery_and_retired_credential_are_rejected() {
    let mut snapshot = snapshot();
    snapshot.delivery.lease_token = Some("lease".to_owned());
    snapshot.credential = Some(SentCredential {
        status: Some("retired".to_owned()),
        ..credential_fixture(&snapshot)
    });
    let admission = snapshot.evaluate(&evidence(), at(7));
    assert!(
        admission
            .blockers
            .contains(&SentAdmissionBlocker::RequiresUnclaimedWriteUnknownOrAdminFailure)
    );
    assert!(admission.blockers.contains(&SentAdmissionBlocker::FrozenCredentialRetiredOrChanged));
    assert!(!admission.eligible);
}

#[test]
fn sent_minute_outside_window_and_missing_seller_reservation_are_rejected() {
    let mut snapshot = snapshot();
    snapshot.funding_allocation = None;
    let mut evidence = evidence();
    evidence.sent_minute = at(3);
    let admission = snapshot.evaluate(&evidence, at(7));
    assert!(admission.blockers.contains(&SentAdmissionBlocker::SentMinuteOutsideFrozenWindow));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::RequiresExactUnknownSellerReservation)
    );
}

#[test]
fn unbound_frozen_identity_and_foreign_scope_are_rejected() {
    let mut snapshot = snapshot();
    let payload = snapshot.delivery.payload.as_mut().expect("fixture payload");
    payload.world = "isekai".to_owned();
    payload.credits = CREDITS + 1;
    let admission = snapshot.evaluate(&evidence(), at(7));
    assert!(admission.blockers.contains(&SentAdmissionBlocker::UnsupportedDeliveryScope));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::FrozenIdentityBodyMoneyOrAssetsMismatch)
    );
}

#[test]
fn missing_expectation_and_prior_unbound_history_fail_closed() {
    let mut snapshot = snapshot();
    snapshot.expectation = None;
    snapshot.prior_unbound = true;
    let admission = snapshot.evaluate(&evidence(), at(7));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::FrozenIdentityBodyMoneyOrAssetsMismatch)
    );
    assert!(admission.requires_attempt_history_acceptance);
}

/// 非结算投递（提醒、账号回信等）的载荷投影为 `None`：只给出闭集阻断项，不能被当作可准入。
#[test]
fn non_settlement_payload_shape_is_rejected_with_scope_blocker() {
    let mut snapshot = snapshot();
    snapshot.delivery.payload = None;
    let admission = snapshot.evaluate(&evidence(), at(7));
    assert!(!admission.eligible);
    assert!(admission.blockers.contains(&SentAdmissionBlocker::UnsupportedDeliveryScope));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::FrozenIdentityBodyMoneyOrAssetsMismatch)
    );
}

/// 货款正文写明装备名后的载荷：名称取自该义务对应的装备行。
fn named_payout_snapshot() -> SentAdmissionSnapshot {
    let mut snapshot = snapshot();
    payload_of(&mut snapshot).variables.item_name = Some(EQUIPMENT_NAME.to_owned());
    snapshot
}

fn payload_of(snapshot: &mut SentAdmissionSnapshot) -> &mut SentPayload {
    snapshot.delivery.payload.as_mut().unwrap_or_else(|| panic!("货款载荷夹具必须存在"))
}

/// 附件现金已被卖家领取后的原生 Sent 观察：剩余为零、正文带领取回执。
fn collected_evidence() -> ManualSentEvidence {
    ManualSentEvidence {
        remaining_credits: 0,
        observed_body_digest: [8; 32],
        collected: true,
        receipt: "Removed attachment: 300,000 Credits".to_owned(),
        ..evidence()
    }
}

/// 带装备名的新货款与旧货款一样可人工确认寄出，已领取与未领取两种证据都准入。
#[test]
fn named_seller_payout_admits_collected_and_uncollected_evidence() {
    for evidence in [evidence(), collected_evidence()] {
        let admission = named_payout_snapshot().evaluate(&evidence, at(7));
        assert_eq!(admission.blockers, Vec::new(), "collected={}", evidence.collected);
        assert!(admission.eligible);
    }
    // 旧货款（不带装备名）在已领取证据下同样准入，形状判定互不影响。
    let admission = snapshot().evaluate(&collected_evidence(), at(7));
    assert_eq!(admission.blockers, Vec::new());
}

/// 反例：期望名称只来自义务对应的装备行，载荷自报的名称不能成为期望值；错误名称、换绑装备、
/// 金额、正文摘要、UID、资金分配或缺失发送 fence 均拒绝。
#[test]
fn named_seller_payout_rejects_any_mismatched_fact() {
    let identity_mismatch = SentAdmissionBlocker::FrozenIdentityBodyMoneyOrAssetsMismatch;
    let mut wrong_name = named_payout_snapshot();
    payload_of(&mut wrong_name).variables.item_name =
        Some("Peerless Ethereal Shortsword of Slaughter".to_owned());
    let mut empty_name = named_payout_snapshot();
    payload_of(&mut empty_name).variables.item_name = Some(String::new());
    let mut foreign_equipment = named_payout_snapshot();
    if let Some(equipment) = foreign_equipment.equipment.as_mut() {
        equipment.id = uuid(23);
    }
    let mut wrong_amount = named_payout_snapshot();
    payload_of(&mut wrong_amount).variables.amount = Some((CREDITS - 1).to_string());
    for (label, snapshot) in [
        ("wrong_name", wrong_name),
        ("empty_name", empty_name),
        ("foreign_equipment", foreign_equipment),
        ("wrong_amount", wrong_amount),
    ] {
        let admission = snapshot.evaluate(&evidence(), at(7));
        assert!(!admission.eligible, "{label}");
        assert!(
            admission.blockers.contains(&identity_mismatch),
            "{label}: {:?}",
            admission.blockers
        );
    }

    let mut wrong_digest = evidence();
    wrong_digest.original_body_digest = [9; 32];
    let admission = named_payout_snapshot().evaluate(&wrong_digest, at(7));
    assert!(admission.blockers.contains(&identity_mismatch));

    let mut wrong_uid = evidence();
    wrong_uid.sender_uid = "9002".to_owned();
    let admission = named_payout_snapshot().evaluate(&wrong_uid, at(7));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::RequiresUniqueReadySenderFundingAccount)
    );
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::RequiresExactUnknownSellerReservation)
    );

    let mut wrong_allocation = named_payout_snapshot();
    if let Some(allocation) = wrong_allocation.funding_allocation.as_mut() {
        allocation.amount = CREDITS + 1;
    }
    let admission = wrong_allocation.evaluate(&evidence(), at(7));
    assert!(
        admission.blockers.contains(&SentAdmissionBlocker::RequiresExactUnknownSellerReservation)
    );

    let mut no_fence = named_payout_snapshot();
    no_fence.sending_fencing_version = 0;
    let admission = no_fence.evaluate(&evidence(), at(7));
    assert!(admission.blockers.contains(&identity_mismatch));
}

/// 修复不扩大人工确认的义务范围：带装备名的买方退款仍按范围外拒绝。
#[test]
fn named_buyer_refund_stays_outside_the_manual_scope() {
    let mut snapshot = named_payout_snapshot();
    if let Some(obligation) = snapshot.obligation.as_mut() {
        obligation.obligation_kind = "buyer_refund".to_owned();
    }
    payload_of(&mut snapshot).template = "settlement_buyer_refund".to_owned();
    let admission = snapshot.evaluate(&evidence(), at(7));
    assert!(!admission.eligible);
    assert!(admission.blockers.contains(&SentAdmissionBlocker::UnsupportedDeliveryScope));
}
