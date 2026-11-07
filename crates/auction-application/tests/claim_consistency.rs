//! 结算履约 claim 一致性的正反例；只覆盖纯判定，不连接数据库。

use auction_application::settlement::{
    SettlementClaimContextBlocker, SettlementClaimContextSnapshot, SettlementClaimExecutionBlocker,
    SettlementClaimExecutionSnapshot,
};
use uuid::Uuid;

fn uuid(seed: u64) -> Uuid {
    Uuid::from_u64_pair(seed, seed)
}

fn pending() -> SettlementClaimExecutionSnapshot {
    SettlementClaimExecutionSnapshot {
        obligation_id: uuid(1),
        execution_state: "pending".to_owned(),
        state_version: 1,
        claim_fencing: 0,
        retry_count: 0,
        dispatch_world: "main".to_owned(),
        delivery_operation: "send".to_owned(),
        delivery_id: None,
        prepare_command_id: None,
        prepare_request_digest: None,
        activate_command_id: None,
        activate_request_digest: None,
        delivery_snapshot_id: None,
    }
}

fn prepared() -> SettlementClaimExecutionSnapshot {
    SettlementClaimExecutionSnapshot {
        execution_state: "prepared".to_owned(),
        delivery_id: Some(uuid(2)),
        prepare_command_id: Some(uuid(3)),
        prepare_request_digest: Some(vec![1; 32]),
        delivery_snapshot_id: Some(uuid(2)),
        ..pending()
    }
}

fn enqueued() -> SettlementClaimExecutionSnapshot {
    SettlementClaimExecutionSnapshot {
        execution_state: "enqueued".to_owned(),
        activate_command_id: Some(uuid(4)),
        activate_request_digest: Some(vec![2; 32]),
        ..prepared()
    }
}

#[test]
fn execution_accepts_exactly_the_three_claim_stages() {
    assert_eq!(pending().evaluate(), Ok(()));
    assert_eq!(prepared().evaluate(), Ok(()));
    assert_eq!(enqueued().evaluate(), Ok(()));

    let mut guard = pending();
    guard.obligation_id = Uuid::nil();
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::MissingObligationIdentity));

    let mut guard = pending();
    guard.execution_state = "settled".to_owned();
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::UnknownExecutionState));
}

#[test]
fn execution_versions_worlds_and_operation_fail_closed() {
    let mut guard = pending();
    guard.state_version = 0;
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::InvalidVersionOrCounters));

    let mut guard = pending();
    guard.claim_fencing = -1;
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::InvalidVersionOrCounters));

    let mut guard = pending();
    guard.retry_count = -1;
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::InvalidVersionOrCounters));

    let mut guard = pending();
    guard.dispatch_world = "arena".to_owned();
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::UnknownDispatchWorld));

    let mut guard = pending();
    guard.delivery_operation = "cancel".to_owned();
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::UnknownDeliveryOperation));
}

#[test]
fn execution_identity_groups_must_be_complete() {
    let mut guard = pending();
    guard.delivery_id = Some(uuid(2));
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::IncompletePrepareIdentity));

    let mut guard = prepared();
    guard.prepare_request_digest = None;
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::IncompletePrepareIdentity));

    let mut guard = prepared();
    guard.activate_command_id = Some(uuid(4));
    assert_eq!(
        guard.evaluate(),
        Err(SettlementClaimExecutionBlocker::IncompleteActivationIdentity)
    );

    let mut guard = pending();
    guard.activate_request_digest = Some(vec![2; 32]);
    assert_eq!(
        guard.evaluate(),
        Err(SettlementClaimExecutionBlocker::IncompleteActivationIdentity)
    );
}

#[test]
fn execution_stage_shape_must_match_its_identities() {
    let mut guard = pending();
    guard.delivery_snapshot_id = Some(uuid(2));
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::ExecutionShapeMismatch));

    let mut guard = prepared();
    guard.delivery_snapshot_id = None;
    assert_eq!(guard.evaluate(), Err(SettlementClaimExecutionBlocker::ExecutionShapeMismatch));

    let mut guard = enqueued();
    guard.activate_request_digest = None;
    assert_eq!(
        guard.evaluate(),
        Err(SettlementClaimExecutionBlocker::IncompleteActivationIdentity)
    );
}

fn context() -> SettlementClaimContextSnapshot {
    SettlementClaimContextSnapshot {
        obligation_world: "main".to_owned(),
        equipment_world: Some("main".to_owned()),
        auction_id: uuid(10),
        lot_id: uuid(11),
        origin_command_id: uuid(12),
        equipment_id: uuid(13),
        external_equipment_id: Some("317632875".to_owned()),
        equipment_name: Some("Legendary Sword".to_owned()),
        recipient_user_id: Some(uuid(14)),
        recipient_user_game_identity: Some("Player".to_owned()),
        recipient_game_identity: None,
        amount: 100,
        carrier_credit: 0,
        obligation_kind: "seller_payout".to_owned(),
        submission_id: None,
        submission_source_kind: None,
        submission_source_reference: None,
        submission_mailbox_key: None,
        submission_external_mail_id: None,
        submission_seller: None,
        prerequisite_obligation_id: None,
        prerequisite_row_id: None,
    }
}

#[test]
fn context_accepts_user_owned_and_frozen_name_recipients() {
    assert_eq!(context().evaluate(), Ok(()));
    let mut guard = context();
    guard.recipient_user_id = None;
    guard.recipient_user_game_identity = None;
    guard.recipient_game_identity = Some("FrozenName".to_owned());
    assert_eq!(guard.evaluate(), Ok(()));
}

#[test]
fn context_world_identity_and_equipment_facts_fail_closed() {
    let mut guard = context();
    guard.equipment_world = Some("isekai".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::WorldMismatch));

    let mut guard = context();
    guard.obligation_world = "arena".to_owned();
    guard.equipment_world = Some("arena".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::WorldMismatch));

    let mut guard = context();
    guard.lot_id = Uuid::nil();
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::MissingIdentity));

    let mut guard = context();
    guard.external_equipment_id = Some(" 317632875".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::EquipmentFactInvalid));

    let mut guard = context();
    guard.equipment_name = Some(String::new());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::EquipmentFactInvalid));
}

#[test]
fn context_recipient_owner_must_be_unambiguous() {
    let mut guard = context();
    guard.recipient_game_identity = Some("FrozenName".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::RecipientOwnerMismatch));

    let mut guard = context();
    guard.recipient_user_id = Some(Uuid::nil());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::RecipientOwnerMismatch));

    let mut guard = context();
    guard.recipient_user_game_identity = None;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::RecipientOwnerMismatch));

    let mut guard = context();
    guard.recipient_user_id = None;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::RecipientOwnerMismatch));
}

#[test]
fn context_amounts_and_prerequisite_rows_are_bounded() {
    let mut guard = context();
    guard.amount = -1;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::AmountInvalid));

    let mut guard = context();
    guard.carrier_credit = -1;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::AmountInvalid));

    let mut guard = context();
    guard.carrier_credit = i64::from(u32::MAX) + 1;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::AmountInvalid));

    let mut guard = context();
    guard.prerequisite_obligation_id = Some(uuid(15));
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::MissingPrerequisiteRow));

    let mut guard = context();
    guard.prerequisite_obligation_id = Some(uuid(15));
    guard.prerequisite_row_id = Some(uuid(15));
    assert_eq!(guard.evaluate(), Ok(()));
}

fn return_context(source_kind: &str) -> SettlementClaimContextSnapshot {
    let mut guard = context();
    guard.obligation_kind = "unsold_return".to_owned();
    guard.recipient_user_id = None;
    guard.recipient_user_game_identity = None;
    guard.recipient_game_identity = Some("Seller".to_owned());
    guard.submission_id = Some(uuid(20));
    guard.submission_seller = Some("Seller".to_owned());
    guard.submission_source_kind = Some(source_kind.to_owned());
    guard
}

#[test]
fn return_context_requires_a_complete_submission_source() {
    let mut mail = return_context("mail");
    mail.submission_mailbox_key = Some("mooglemail:main".to_owned());
    mail.submission_external_mail_id = Some("3120869".to_owned());
    assert_eq!(mail.evaluate(), Ok(()));

    let mut admin = return_context("admin");
    admin.submission_source_reference = Some("manual-entry".to_owned());
    assert_eq!(admin.evaluate(), Ok(()));

    let mut guard = return_context("mail");
    guard.submission_mailbox_key = Some("mooglemail:isekai".to_owned());
    guard.submission_external_mail_id = Some("3120869".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::ReturnSubmissionSourceInvalid));

    let mut guard = return_context("admin");
    guard.submission_external_mail_id = Some("3120869".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::ReturnSubmissionSourceInvalid));

    let mut guard = return_context("mail");
    guard.submission_id = None;
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::MissingReturnSubmission));

    let mut guard = return_context("mail");
    guard.submission_seller = Some("Another".to_owned());
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::MissingReturnSubmission));
}

#[test]
fn unknown_obligation_kind_is_isolated_not_guessed() {
    let mut guard = context();
    guard.obligation_kind = "mystery_payout".to_owned();
    assert_eq!(guard.evaluate(), Err(SettlementClaimContextBlocker::UnknownObligationKind));
}
