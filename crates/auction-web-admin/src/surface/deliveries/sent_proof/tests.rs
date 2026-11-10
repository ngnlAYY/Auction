use auction_contracts::{
    AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
    AdminDeliveryEvidenceResponse, AdminDeliveryResponse,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

use super::evidence::{decode_hex_digest, selectable_evidence};
use super::state::{SentProofDraft, SentProofIssue};
use super::validation::{build_evidence, sent_minute_from_input, sent_minute_from_timestamp};

fn time(minute: u32) -> String {
    Utc.with_ymd_and_hms(2026, 11, 2, 1, minute, 0).single().expect("timestamp").to_rfc3339()
}

fn evidence() -> AdminDeliveryEvidenceResponse {
    AdminDeliveryEvidenceResponse {
        reference: "mailbox_sweep:3125345".to_owned(),
        kind: "mailbox_sweep".to_owned(),
        external_mail_id: Some("3125345".to_owned()),
        observed_at: Some(time(2)),
        sent_at: Some(time(1)),
        recipient: Some("Restia".to_owned()),
        subject: Some("Auction settlement".to_owned()),
        body_digest: Some("cd".repeat(32)),
        collected: Some(false),
        cod_pending: Some(0),
        remaining_credits: Some(300_000),
        remaining_equipment_count: Some(0),
        receipt: None,
    }
}

fn detail() -> AdminDeliveryDetailResponse {
    AdminDeliveryDetailResponse {
        delivery: AdminDeliveryResponse {
            id: Uuid::from_bytes([1; 16]).to_string(),
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
            completed_at: Some(time(2)),
            external_reference: None,
            failure_code: Some("write_unknown".to_owned()),
            reason_code: None,
            take_required: false,
            returned_mail_id: None,
            created_at: time(0),
            updated_at: time(2),
        },
        attempts: vec![AdminDeliveryAttemptResponse {
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
        }],
        evidence: vec![evidence()],
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

fn filled_draft(detail: &AdminDeliveryDetailResponse) -> SentProofDraft {
    let mut draft = SentProofDraft::for_evidence(selectable_evidence(detail).first().copied());
    draft.sender_name = "Restia".to_owned();
    draft.sender_uid = "12345".to_owned();
    draft.reason = "verified in Sent".to_owned();
    draft
}

#[test]
fn decodes_lowercase_hex_digest_only() {
    assert_eq!(decode_hex_digest(&"ab".repeat(32)), Some([0xab; 32]));
    assert_eq!(decode_hex_digest(&"AB".repeat(32)), None);
    assert_eq!(decode_hex_digest("ab"), None);
    assert_eq!(decode_hex_digest(&"zz".repeat(32)), None);
}

#[test]
fn normalizes_sent_minute_to_utc_zero_seconds() {
    assert_eq!(sent_minute_from_input("2026-11-02T01:14"), Some("2026-11-02T01:14:00Z".to_owned()));
    assert_eq!(sent_minute_from_input(""), None);
    assert_eq!(
        sent_minute_from_timestamp("2026-11-02T01:14:37Z"),
        Some("2026-11-02T01:14:00Z".to_owned())
    );
}

#[test]
fn builds_evidence_from_frozen_attempt_and_observation() {
    let detail = detail();
    let built = build_evidence(&detail, &filled_draft(&detail)).expect("evidence");
    assert_eq!(built.schema_version, 1);
    assert_eq!(built.world_id, "main");
    assert_eq!(built.external_mail_id, "3125345");
    assert_eq!(built.sender_uid, "12345");
    assert_eq!(built.recipient, "Restia");
    assert_eq!(built.sent_minute, "2026-11-02T01:01:00Z");
    assert_eq!(built.original_body_digest, [0xab; 32]);
    assert_eq!(built.observed_body_digest, [0xcd; 32]);
    assert_eq!(built.remaining_credits, 300_000);
    assert!(built.equipment_ids.is_empty());
}

#[test]
fn rejects_draft_without_recorded_evidence_or_sender() {
    let detail = detail();
    let mut draft = filled_draft(&detail);
    draft.evidence_reference = "mailbox_sweep:999".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::Evidence));

    let mut draft = filled_draft(&detail);
    draft.sender_uid = "0".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::SenderUid));

    let mut draft = filled_draft(&detail);
    draft.sender_name = "  ".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::SenderName));
}

#[test]
fn rejects_unselectable_kind_and_missing_digest() {
    let mut detail = self::detail();
    detail.evidence[0].kind = "compose_discard".to_owned();
    assert!(selectable_evidence(&detail).is_empty());
    assert_eq!(
        build_evidence(&detail, &filled_draft(&self::detail())),
        Err(SentProofIssue::Evidence)
    );

    let mut detail = self::detail();
    detail.evidence[0].body_digest = None;
    assert_eq!(
        build_evidence(&detail, &filled_draft(&self::detail())),
        Err(SentProofIssue::Digest)
    );
}

#[test]
fn collected_draft_drops_frozen_equipment_ids() {
    let mut detail = detail();
    detail.attempts[0].credits = Some(0);
    detail.attempts[0].cod = Some(0);
    detail.attempts[0].equipment_ids = vec!["300753957".to_owned()];
    detail.evidence[0].remaining_equipment_count = Some(0);
    let mut draft = filled_draft(&detail);
    draft.collected = true;

    let built = build_evidence(&detail, &draft).expect("evidence");
    assert!(built.collected);
    assert!(built.equipment_ids.is_empty());

    let mut uncollected = draft;
    uncollected.collected = false;
    let built = build_evidence(&detail, &uncollected).expect("evidence");
    assert_eq!(built.equipment_ids, vec!["300753957".to_owned()]);
}

#[test]
fn draft_preserves_operator_fields_across_evidence_switch() {
    let detail = detail();
    let draft = filled_draft(&detail);
    let next = draft.with_evidence(selectable_evidence(&detail).first().copied());
    assert_eq!(next.sender_uid, "12345");
    assert_eq!(next.reason, "verified in Sent");
    assert_eq!(next.remaining_credits, "300000");
}
