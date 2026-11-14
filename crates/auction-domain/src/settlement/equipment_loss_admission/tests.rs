//! 装备损失人工处置准入的回归用例。

use super::*;

fn uuid(seed: u64) -> Uuid {
    Uuid::from_u64_pair(seed, seed)
}

fn obligation(
    id: u64,
    kind: &str,
    state: &str,
    amount: i64,
    prerequisite: Option<u64>,
) -> EquipmentLossObligation {
    EquipmentLossObligation {
        id: uuid(id),
        obligation_kind: kind.to_owned(),
        business_state: state.to_owned(),
        amount,
        prerequisite_obligation_id: prerequisite.map(uuid),
        lot_id: uuid(20),
        auction_id: uuid(21),
        equipment_id: uuid(22),
        world_id: "main".to_owned(),
        recipient_user_id: Some("1001".to_owned()),
    }
}

fn fulfillment(state: &str) -> EquipmentLossFulfillment {
    EquipmentLossFulfillment {
        execution_state: state.to_owned(),
        state_version: 1,
        delivery_id: None,
        prepare_command_id: None,
        activate_command_id: None,
        prepare_request_digest: None,
        activate_request_digest: None,
        claim_token: None,
        claimed_at: None,
        claim_expires_at: None,
        proof_kind: None,
        proof_reference: None,
        proof_digest: None,
        proof_observed_at: None,
    }
}

fn admission() -> EquipmentLossAdmission {
    EquipmentLossAdmission {
        auction: EquipmentLossAuction {
            id: uuid(21),
            world_id: "main".to_owned(),
            status: "settled".to_owned(),
            settled_at: Some(chrono::Utc::now()),
        },
        lot: EquipmentLossLot {
            id: uuid(20),
            auction_id: uuid(21),
            equipment_id: uuid(22),
            status: "sold".to_owned(),
            leader_user_id: Some("1001".to_owned()),
        },
        equipment: EquipmentLossEquipment {
            id: uuid(22),
            world_id: "main".to_owned(),
            external_equipment_id: "300753957".to_owned(),
        },
        settlement: EquipmentLossSettlement {
            buyer_amount: 5000,
            compensation_amount: 1000,
            seller_amount: 6000,
            tax_amount: 0,
        },
        recovery: EquipmentLossRecovery {
            id: uuid(23),
            lot_id: uuid(20),
            auction_id: uuid(21),
            equipment_id: uuid(22),
            payment_obligation_id: uuid(1),
            winner_user_id: Some("1001".to_owned()),
            world_id: "main".to_owned(),
            stage: "awaiting_payment".to_owned(),
            state_version: 3,
            cod_return_count: 0,
            reminder_count: 0,
            resolution: None,
            equipment_obligation_id: None,
            return_mail_id: None,
            return_external_mail_id: None,
            custody_proof_kind: None,
            custody_proof_reference: None,
            custody_proof_digest: None,
            custody_proof_observed_at: None,
            relisted_auction_id: None,
            relisted_lot_id: None,
            blocking_code: None,
            manual_reconcile_delivery_id: None,
            completed_at: None,
        },
        payment: obligation(1, "winner_equipment_cod", "ready", 5000, None),
        payment_fulfillment: {
            let mut fulfillment = fulfillment("manual_reconcile");
            fulfillment.delivery_id = Some(uuid(30));
            fulfillment.prepare_command_id = Some(uuid(31));
            fulfillment.activate_command_id = Some(uuid(32));
            fulfillment.prepare_request_digest = Some("\\x01".to_owned());
            fulfillment.activate_request_digest = Some("\\x02".to_owned());
            fulfillment
        },
        seller: obligation(2, "seller_payout", "blocked", 6000, Some(1)),
        seller_fulfillment: fulfillment("pending"),
        refund: obligation(3, "buyer_refund", "contingent", 5000, Some(1)),
        refund_fulfillment: fulfillment("pending"),
        delivery: EquipmentLossDelivery {
            id: uuid(30),
            status: "manual_reconcile".to_owned(),
            attempt_count: 1,
            fencing_version: 1,
            failure_code: Some("not_observed".to_owned()),
            origin_kind: "settlement".to_owned(),
            origin_id: uuid(1).to_string(),
            command_id: uuid(31),
            request_digest: Some("\\x01".to_owned()),
            activation_command_id: Some(uuid(32)),
            activation_request_digest: Some("\\x02".to_owned()),
            external_reference: None,
            lease_token: None,
            lease_expires_at: None,
            result_metadata: EquipmentLossResultMetadata {
                operation: Some("inventory".to_owned()),
                stage: Some("read".to_owned()),
                write_started: Some(false),
                proof: None,
                sent_verified: None,
            },
            payload: EquipmentLossDeliveryPayload {
                schema_version: 1,
                operation: "send".to_owned(),
                world: "main".to_owned(),
                template: "settlement_winner_equipment_cod".to_owned(),
                credits: 0,
                cod: 5000,
                equipment_secret_refs: vec![EquipmentLossSecretRef {
                    kind: "auction_equipment_key".to_owned(),
                    id: uuid(22),
                }],
            },
        },
        funding_accounts: vec![EquipmentLossFundingAccount {
            uid: "9001".to_owned(),
            status: "ready".to_owned(),
            accounting_mode: "record_only".to_owned(),
        }],
        conflict: false,
    }
}

#[test]
fn frozen_graph_admits_and_keeps_operator_split() {
    let admission = admission();
    assert_eq!(admission.evaluate(), Vec::new());
    let split = admission.funding_split().expect("冻结金额可装配");
    assert_eq!(split.buyer.value(), 5000);
    assert_eq!(split.compensation.value(), 1000);
    assert_eq!(split.seller.value(), 6000);
    assert_eq!(split.tax.value(), 0);
}

#[test]
fn every_guard_rejects_independently() {
    let cases: Vec<(EquipmentLossAdmission, EquipmentLossBlocker)> = vec![
        (
            EquipmentLossAdmission {
                funding_accounts: vec![EquipmentLossFundingAccount {
                    uid: "9001".to_owned(),
                    status: "ready".to_owned(),
                    accounting_mode: "ledger".to_owned(),
                }],
                ..admission()
            },
            EquipmentLossBlocker::UniqueReadyRecordOnlyAccount,
        ),
        (
            EquipmentLossAdmission {
                auction: EquipmentLossAuction { settled_at: None, ..admission().auction },
                ..admission()
            },
            EquipmentLossBlocker::MainSettledSoldLot,
        ),
        (
            EquipmentLossAdmission {
                seller: obligation(2, "seller_payout", "ready", 6000, Some(1)),
                ..admission()
            },
            EquipmentLossBlocker::FrozenObligationMismatch,
        ),
        (
            EquipmentLossAdmission {
                lot: EquipmentLossLot {
                    leader_user_id: Some("2002".to_owned()),
                    ..admission().lot
                },
                ..admission()
            },
            EquipmentLossBlocker::OwnerIdentityMismatch,
        ),
        (
            EquipmentLossAdmission {
                recovery: EquipmentLossRecovery { reminder_count: 1, ..admission().recovery },
                ..admission()
            },
            EquipmentLossBlocker::RecoveryAlreadyActed,
        ),
        (
            EquipmentLossAdmission {
                seller_fulfillment: {
                    let mut fulfillment = fulfillment("pending");
                    fulfillment.claim_token = Some("claim".to_owned());
                    fulfillment
                },
                ..admission()
            },
            EquipmentLossBlocker::FulfillmentAlreadyActedOrClaimed,
        ),
        (
            EquipmentLossAdmission {
                delivery: EquipmentLossDelivery {
                    result_metadata: EquipmentLossResultMetadata {
                        write_started: Some(true),
                        ..admission().delivery.result_metadata
                    },
                    ..admission().delivery
                },
                ..admission()
            },
            EquipmentLossBlocker::OriginalAttemptNotProvenInventoryOnly,
        ),
        (
            EquipmentLossAdmission {
                delivery: EquipmentLossDelivery {
                    payload: EquipmentLossDeliveryPayload {
                        cod: 1,
                        ..admission().delivery.payload
                    },
                    ..admission().delivery
                },
                ..admission()
            },
            EquipmentLossBlocker::OriginalDeliveryPayloadMismatch,
        ),
        (
            EquipmentLossAdmission { conflict: true, ..admission() },
            EquipmentLossBlocker::ExistingDispositionOrFunding,
        ),
        (
            EquipmentLossAdmission {
                settlement: EquipmentLossSettlement { seller_amount: 0, ..admission().settlement },
                ..admission()
            },
            EquipmentLossBlocker::InvalidFrozenAmounts,
        ),
    ];
    for (admission, expected) in cases {
        let blockers = admission.evaluate();
        assert!(blockers.contains(&expected), "缺少阻断项 {:?}，实际 {:?}", expected, blockers);
    }
}
