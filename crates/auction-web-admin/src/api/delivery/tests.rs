//! Admin 投递 decoder 与详情校验的回归用例。

use auction_contracts::{
    AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
    AdminDeliveryDispositionResponse, AdminDeliveryEvidenceResponse, AdminDeliveryResponse,
    AdminDeliverySentProofPreviewResponse, AdminDeliverySentProofResponse,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use super::{decode::StrictDeliveryEnvelope, decode_confirmation, decode_detail, decode_preview};

fn time(offset: u32) -> String {
    Utc.with_ymd_and_hms(2026, 11, 2, 1, offset, 0).single().expect("timestamp").to_rfc3339()
}

// `valid_uuid` 会校验版本位与变体位，测试夹具必须是规范 UUID。
const DELIVERY_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const OTHER_ID: &str = "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60";

fn delivery() -> AdminDeliveryResponse {
    AdminDeliveryResponse {
        id: DELIVERY_ID.to_owned(),
        origin_kind: "settlement".to_owned(),
        origin_id: "42".to_owned(),
        recipient: "Restia".to_owned(),
        cod: Some(0),
        status: "manual_reconcile".to_owned(),
        template: Some("settlement_seller_payout".to_owned()),
        category: Some("settlement".to_owned()),
        world: Some("main".to_owned()),
        retirement_reason: None,
        suppression_reason: None,
        lease_expires_at: None,
        fencing_version: 3,
        attempt_count: 1,
        claimed_at: Some(time(0)),
        sending_at: Some(time(0)),
        completed_at: Some(time(1)),
        external_reference: None,
        failure_code: Some("write_unknown".to_owned()),
        reason_code: None,
        take_required: false,
        returned_mail_id: None,
        created_at: time(0),
        updated_at: time(1),
    }
}

fn attempt() -> AdminDeliveryAttemptResponse {
    AdminDeliveryAttemptResponse {
        operation: "send".to_owned(),
        fencing_version: 3,
        world_id: "main".to_owned(),
        sending_at: time(0),
        recipient: Some("Restia".to_owned()),
        subject: Some("Auction settlement".to_owned()),
        body_digest: Some("ab".repeat(32)),
        credits: Some(300_000),
        cod: Some(0),
        equipment_ids: Vec::new(),
        external_mail_id: None,
        credential: None,
        bound_sent_mail_id: None,
    }
}

fn detail() -> AdminDeliveryDetailResponse {
    AdminDeliveryDetailResponse {
        delivery: delivery(),
        attempts: vec![attempt()],
        evidence: vec![AdminDeliveryEvidenceResponse {
            reference: "mailbox_sweep:3125345".to_owned(),
            kind: "mailbox_sweep".to_owned(),
            external_mail_id: Some("3125345".to_owned()),
            observed_at: Some(time(2)),
            sent_at: None,
            recipient: Some("Restia".to_owned()),
            subject: Some("Auction settlement".to_owned()),
            body_digest: Some("cd".repeat(32)),
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            remaining_equipment_count: Some(0),
            receipt: None,
        }],
        custody: AdminDeliveryCustodyResponse {
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            receipt: None,
            observed_at: Some(time(2)),
            operator_confirmed_collected: false,
        },
        dispositions: Vec::new(),
        obligation: None,
        recovery: None,
    }
}

#[test]
fn accepts_complete_detail() {
    let decoded = decode_detail(StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "loaded".to_owned(),
        data: detail(),
    })
    .expect("decode");
    assert_eq!(decoded.attempts.len(), 1);
}

#[test]
fn accepts_operator_acknowledged_disposition_in_detail() {
    let mut data = detail();
    data.dispositions.push(AdminDeliveryDispositionResponse {
        command_id: OTHER_ID.to_owned(),
        kind: "operator_acknowledged".to_owned(),
        occurred_at: time(2),
        status: None,
        fencing_version: Some(3),
        failure_code: None,
        external_reference: None,
        actor: Some("admin".to_owned()),
        reason: Some("handled outside the outbox".to_owned()),
    });
    let decoded = decode_detail(StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "loaded".to_owned(),
        data,
    })
    .expect("operator_acknowledged must remain decodable");
    assert_eq!(decoded.dispositions[0].kind, "operator_acknowledged");
}

#[test]
fn accepts_unified_disposition_kinds_in_detail() {
    let mut data = detail();
    data.dispositions.push(AdminDeliveryDispositionResponse {
        command_id: OTHER_ID.to_owned(),
        kind: "auto_sent_proof".to_owned(),
        occurred_at: time(2),
        status: None,
        fencing_version: Some(3),
        failure_code: Some("sent_recovered_frozen_v1".to_owned()),
        external_reference: None,
        actor: Some("delivery-worker".to_owned()),
        reason: None,
    });
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_ok()
    );
}

#[test]
fn rejects_send_attempt_without_frozen_fields() {
    let mut data = detail();
    data.attempts[0].subject = None;
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_unknown_evidence_kind_and_uppercase_digest() {
    let mut data = detail();
    data.evidence[0].kind = "draft_discard".to_owned();
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );

    let mut data = detail();
    data.evidence[0].body_digest = Some("AB".repeat(32));
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_take_attempt_with_send_fields_only() {
    let mut data = detail();
    data.attempts[0] = AdminDeliveryAttemptResponse {
        operation: "take".to_owned(),
        external_mail_id: None,
        ..attempt()
    };
    assert!(
        decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data,
        })
        .is_err()
    );
}

#[test]
fn rejects_preview_for_other_delivery_or_eligible_with_blockers() {
    let preview = |delivery_id: &str, eligible: bool, blockers: Vec<&str>| {
        AdminDeliverySentProofPreviewResponse {
            eligible,
            blockers: blockers.into_iter().map(str::to_owned).collect(),
            delivery_id: delivery_id.to_owned(),
            expected_fencing_version: 3,
            external_reference: "mooglemail:main:send:3125345".to_owned(),
            requires_collected_asset_acceptance: false,
            requires_attempt_history_acceptance: false,
            corrects_admin_failure: true,
        }
    };
    let envelope = |data| StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "preview".to_owned(),
        data,
    };
    assert!(decode_preview(envelope(preview(DELIVERY_ID, true, Vec::new())), DELIVERY_ID).is_ok());
    assert!(decode_preview(envelope(preview(OTHER_ID, true, Vec::new())), DELIVERY_ID).is_err());
    assert!(
        decode_preview(
            envelope(preview(DELIVERY_ID, true, vec!["sender_player_name_mismatch"])),
            DELIVERY_ID
        )
        .is_err()
    );
}

#[test]
fn confirmation_must_echo_command_and_delivery() {
    let command_id = Uuid::from_bytes([9; 16]);
    let data = AdminDeliverySentProofResponse {
        command_id: command_id.to_string(),
        delivery_id: DELIVERY_ID.to_owned(),
        external_reference: "mooglemail:main:send:3125345".to_owned(),
        replayed: false,
    };
    let envelope = || StrictDeliveryEnvelope {
        code: "ok".to_owned(),
        message: "confirmed".to_owned(),
        data: data.clone(),
    };
    assert!(decode_confirmation(envelope(), command_id, DELIVERY_ID).is_ok());
    assert!(decode_confirmation(envelope(), Uuid::from_bytes([8; 16]), DELIVERY_ID).is_err());
    assert!(decode_confirmation(envelope(), command_id, OTHER_ID).is_err());
}
