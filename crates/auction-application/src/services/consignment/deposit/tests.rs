//! 寄售第 4 期用例与内存假实现的行为测试。

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation,
    LockedEnvelopeFacts, ReservationBinding, ReservationState, SenderIdentity,
};
use auction_domain::consignment::deposit::{QuarantineReason, ReleaseKind};
use auction_domain::consignment::{
    CalendarVersion, FeeBps, FrozenSeason, ListingEquipmentIdentity, ListingState, ListingVersion,
    OriginalMailKey, SeasonState,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::{
    ApplyMailProofInput, DepositCommandService, ReleaseQuarantineAdminInput,
    StartDispositionWriteInput,
};
use crate::ports::consignment::deposit::test_support::FakeDepositStore;
use crate::ports::consignment::{
    AdminDepositAction, AdminDepositQuery, DispositionAction, EnvelopeDisposition, MailProofKind,
    OwnedDepositInstructions, ProofDisposition, ReleaseQuarantineCommand, SeasonNoticeKind,
    StartReceipt, WriteStart,
};
use crate::ports::consignment::{
    ConsignmentError, DepositBusinessRejection, DepositRead, EnvelopeStore, ReservationStore,
};

const SELLER: u128 = 7;
const LISTING: u128 = 1;

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..4 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("内存假实现未在预期轮数内完成")
}

fn seller() -> UserId {
    UserId::from_uuid(Uuid::from_u128(SELLER))
}

fn client_id(value: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | value))
}

fn version(value: u64) -> ListingVersion {
    ListingVersion::new(value).expect("测试版本")
}

fn setup() -> (Arc<FakeDepositStore>, DepositCommandService) {
    let fake = Arc::new(FakeDepositStore::new());
    let service = DepositCommandService::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
    );
    (fake, service)
}

fn seed_awaiting(fake: &FakeDepositStore, listing_id: Uuid) {
    fake.seed_listing(listing_id, seller(), "42", ListingState::AwaitingDeposit, 1, 100);
}

fn season(clock: Timestamp) -> FrozenSeason {
    FrozenSeason {
        world: SettlementWorld::Isekai,
        season_id: "2026-11-01".to_owned(),
        start_utc: clock - auction_time::Duration::hours(1),
        end_utc: clock + auction_time::Duration::hours(1),
        admission_cutoff_utc: clock,
        calendar_version: CalendarVersion::from_bytes([0xaa; 32]),
        preclose_days: 3,
    }
}

#[test]
fn original_mail_key_uid_uses_the_same_rule_as_external_ids() {
    let boundary = i64::MAX.to_string();
    let inputs = [
        "1",
        "42",
        boundary.as_str(),
        "",
        "0",
        "00",
        "042",
        "+42",
        "-42",
        " 42",
        "42 ",
        "4.2",
        "４２",
        "9223372036854775808",
    ];
    for input in inputs {
        let application = crate::ports::settlement::valid_external_positive_id(input);
        let domain = OriginalMailKey::new(input, SettlementWorld::Main, "1").is_ok();
        assert_eq!(domain, application, "uid {input:?}");
        let domain_mid = OriginalMailKey::new("1", SettlementWorld::Main, input).is_ok();
        assert_eq!(domain_mid, application, "mid {input:?}");
    }
}

#[test]
fn same_id_and_request_replays_the_first_receipt() {
    let (fake, service) = setup();
    seed_awaiting(&fake, Uuid::from_u128(LISTING));
    let command_id = client_id(1);
    let first = run(service.issue_reservation(
        seller(),
        command_id,
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ))
    .expect("首次签发");
    let replay = run(service.issue_reservation(
        seller(),
        command_id,
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ))
    .expect("同请求重放");
    assert_eq!(first, replay);
    assert_eq!(fake.snapshot().reservations.len(), 1);
    assert_eq!(fake.snapshot().listings.get(&Uuid::from_u128(LISTING)).expect("挂单").version, 2);
}

#[test]
fn quarantine_first_receipt_is_permanent_and_observations_are_append_only() {
    let (fake, service) = setup();
    let key = OriginalMailKey::new("42", SettlementWorld::Main, "400").expect("原信键");
    let observation = EnvelopeObservation {
        observation_id: Uuid::from_u128(90),
        session_binding_digest: [1; 32],
        manifest_digest: [2; 32],
        parser_version: "p1".to_owned(),
        subject_is_consign: true,
        sender_identity: SenderIdentity::Unverified,
        list_detail_consistent: true,
        asset_count: Some(0),
        duplicate_raw_eid: false,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: false,
        takeable: Some(false),
        system_rts: false,
        legacy_auction_owner: None,
        items: Vec::new(),
    };
    let facts = LockedEnvelopeFacts {
        mail_key: Some(key),
        observation: observation.clone(),
        reservations: Vec::new(),
        envelope_seller_user_id: None,
        capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
        read_failure: false,
        already_decided: false,
        now: Timestamp::UNIX_EPOCH,
    };
    let first = run(service.decide_envelope(facts.clone())).expect("首次决定").expect("应持久隔离");
    assert_eq!(first.disposition.as_str(), EnvelopeDisposition::Quarantined.as_str());
    let replay = run(service.decide_envelope(facts)).expect("首回执重放").expect("应返回原隔离");
    assert_eq!(first, replay);
    assert_eq!(fake.snapshot().quarantines.len(), 1);
    let quarantine_id = first.quarantine.expect("隔离引用").quarantine_id;
    let updated = EnvelopeObservation {
        sender_identity: SenderIdentity::Verified,
        manifest_digest: [3; 32],
        parser_version: "p2".to_owned(),
        ..observation
    };
    let appended = run(service.append_quarantine_observation(
        quarantine_id,
        "obs-1".to_owned(),
        Timestamp::UNIX_EPOCH,
        updated.clone(),
        None,
    ))
    .expect("追加观察");
    assert_eq!(appended.quarantine_version, 2);
    let replay = run(service.append_quarantine_observation(
        quarantine_id,
        "obs-1".to_owned(),
        Timestamp::UNIX_EPOCH,
        updated,
        None,
    ))
    .expect("同观察重放");
    assert_eq!(appended, replay);
    let detail = run(fake.quarantine(AdminDepositQuery { quarantine_id, cursor: None, limit: 10 }))
        .expect("隔离详情")
        .expect("存在");
    assert_eq!(detail.observations.len(), 2);
    assert_eq!(detail.version, 2);
}

#[test]
fn unproven_take_capability_quarantines_without_consuming() {
    let cases: [(u128, Option<bool>, bool); 3] =
        [(0, Some(true), false), (1, Some(false), true), (2, None, true)];
    for (index, takeable, take_ready) in cases {
        let (fake, service) = setup();
        let listing_id = Uuid::from_u128(LISTING);
        seed_awaiting(&fake, listing_id);
        let expires_at = Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1);
        let (reservation_id, _) = fake.seed_reservation(listing_id, seller(), "42", expires_at);
        let facts = LockedEnvelopeFacts {
            mail_key: Some(
                OriginalMailKey::new("42", SettlementWorld::Main, &format!("{}", 700 + index))
                    .expect("测试原信键"),
            ),
            observation: EnvelopeObservation {
                observation_id: Uuid::from_u128(90 + index),
                session_binding_digest: [1; 32],
                manifest_digest: [2; 32],
                parser_version: "p1".to_owned(),
                subject_is_consign: true,
                sender_identity: SenderIdentity::Verified,
                list_detail_consistent: true,
                asset_count: Some(1),
                duplicate_raw_eid: false,
                canonical_alias_ambiguous: false,
                unknown_attachment: false,
                credits: Some(0),
                cod: Some(0),
                returnable: true,
                takeable,
                system_rts: false,
                legacy_auction_owner: None,
                items: vec![EnvelopeItemObservation {
                    ordinal: 1,
                    kind: EnvelopeItemKind::Equipment,
                    display_name: Some("Fake Gear".to_owned()),
                    raw_eid: Some("42".to_owned()),
                    canonical_eid: Some("42".to_owned()),
                    amount: None,
                    world: Some(SettlementWorld::Main),
                    seller_user_id: Some(seller()),
                    tradeable: true,
                    history_ok: true,
                    protected_ref: Some(Uuid::from_u128(900 + index)),
                }],
            },
            reservations: vec![ReservationBinding {
                reservation_id,
                generation: 7,
                listing_id,
                listing_version: version(1),
                seller_user_id: seller(),
                equipment: ListingEquipmentIdentity::new("main", "42").expect("测试装备"),
                expires_at,
                state: ReservationState::Active,
            }],
            envelope_seller_user_id: Some(seller()),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        };
        let receipt = run(service.decide_envelope(facts)).expect("决定").expect("应持久隔离");
        assert_eq!(receipt.disposition.as_str(), EnvelopeDisposition::Quarantined.as_str());
        let quarantine_id = receipt.quarantine.expect("隔离引用").quarantine_id;
        let detail =
            run(fake.quarantine(AdminDepositQuery { quarantine_id, cursor: None, limit: 10 }))
                .expect("隔离详情")
                .expect("存在");
        assert_eq!(detail.reason, QuarantineReason::TakeUnproven);
        let snapshot = fake.snapshot();
        assert_eq!(
            snapshot.reservations.get(&reservation_id).expect("预约").state,
            ReservationState::Active
        );
        assert_eq!(
            snapshot.listings.get(&listing_id).expect("挂单").state,
            ListingState::AwaitingDeposit
        );
        assert_eq!(snapshot.terminals.len(), 1);
        assert_eq!(fake.counters().permits_issued, 0);
        assert_eq!(fake.counters().posts_recorded, 0);
    }
}

#[test]
fn same_id_with_a_different_request_is_a_conflict() {
    let (fake, service) = setup();
    seed_awaiting(&fake, Uuid::from_u128(LISTING));
    let command_id = client_id(2);
    run(service.issue_reservation(
        seller(),
        command_id,
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ))
    .expect("首次签发");
    let conflict = run(service.issue_reservation(
        seller(),
        command_id,
        Uuid::from_u128(LISTING),
        version(2),
        None,
    ));
    assert_eq!(conflict, Err(ConsignmentError::CommandIdConflict));
}

#[test]
fn reissue_rotates_the_token_but_reprice_preserves_it() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    seed_awaiting(&fake, listing_id);
    run(service.issue_reservation(seller(), client_id(3), listing_id, version(1), None))
        .expect("首次签发");
    let first_token = fake.active_token(listing_id).expect("首个令牌");
    fake.reprice_listing(listing_id, 101);
    assert_eq!(fake.active_token(listing_id), Some(first_token.clone()));
    run(service.issue_reservation(seller(), client_id(4), listing_id, version(3), None))
        .expect("换发");
    let second_token = fake.active_token(listing_id).expect("第二个令牌");
    assert_ne!(first_token, second_token);
    assert_eq!(
        fake.snapshot()
            .reservations
            .values()
            .filter(|reservation| {
                reservation.state == auction_domain::consignment::deposit::ReservationState::Active
            })
            .count(),
        1
    );
}

#[test]
fn disposition_write_issues_exactly_one_permit_per_work() {
    let (fake, service) = setup();
    let decision = Uuid::from_u128(8);
    let delivery = Uuid::from_u128(9);
    let work = Uuid::from_u128(10);
    let first = run(service.start_disposition_write(StartDispositionWriteInput {
        final_decision_id: decision,
        action: DispositionAction::Take,
        delivery_id: delivery,
        fence: 7,
        work_id: work,
        attempt_id: Uuid::from_u128(11),
        claim_token: Uuid::from_u128(12),
        session_binding_digest: [0x66; 32],
        expected_manifest_digest: [0x77; 32],
    }))
    .expect("首次起写");
    let WriteStart::FirstCommitted { receipt: first_receipt, permit } = first else {
        panic!("首次必须携带许可");
    };
    assert!(fake.record_post(permit));
    let replay = run(service.start_disposition_write(StartDispositionWriteInput {
        final_decision_id: decision,
        action: DispositionAction::Take,
        delivery_id: delivery,
        fence: 7,
        work_id: work,
        attempt_id: Uuid::from_u128(11),
        claim_token: Uuid::from_u128(12),
        session_binding_digest: [0x66; 32],
        expected_manifest_digest: [0x77; 32],
    }))
    .expect("重放");
    match replay {
        WriteStart::AlreadyCommitted(receipt) => assert_eq!(receipt, first_receipt),
        WriteStart::FirstCommitted { .. } => panic!("重放不得再发许可"),
    }
    let new_fence = run(service.start_disposition_write(StartDispositionWriteInput {
        final_decision_id: decision,
        action: DispositionAction::Take,
        delivery_id: delivery,
        fence: 8,
        work_id: work,
        attempt_id: Uuid::from_u128(13),
        claim_token: Uuid::from_u128(14),
        session_binding_digest: [0x66; 32],
        expected_manifest_digest: [0x77; 32],
    }))
    .expect("换 fence");
    assert!(matches!(new_fence, WriteStart::AlreadyCommitted(_)));
    let counters = fake.counters();
    assert_eq!(counters.permits_issued, 1);
    assert_eq!(counters.posts_recorded, 1);

    fake.lose_next_response();
    let lost = run(service.start_disposition_write(StartDispositionWriteInput {
        final_decision_id: decision,
        action: DispositionAction::Take,
        delivery_id: delivery,
        fence: 9,
        work_id: Uuid::from_u128(20),
        attempt_id: Uuid::from_u128(21),
        claim_token: Uuid::from_u128(22),
        session_binding_digest: [0x88; 32],
        expected_manifest_digest: [0x99; 32],
    }));
    assert!(matches!(lost, Err(ConsignmentError::OutcomeUnknown)));
    let before_retry = fake.counters().permits_issued;
    let retried = run(service.start_disposition_write(StartDispositionWriteInput {
        final_decision_id: decision,
        action: DispositionAction::Take,
        delivery_id: delivery,
        fence: 9,
        work_id: Uuid::from_u128(20),
        attempt_id: Uuid::from_u128(21),
        claim_token: Uuid::from_u128(22),
        session_binding_digest: [0x88; 32],
        expected_manifest_digest: [0x99; 32],
    }))
    .expect("提交未知后重试");
    assert!(matches!(retried, WriteStart::AlreadyCommitted(_)));
    assert_eq!(fake.counters().permits_issued, before_retry);
    assert_eq!(fake.counters().posts_recorded, 1);
}

#[test]
fn quarantine_release_race_has_at_most_one_owner() {
    let (fake, service) = setup();
    let key = OriginalMailKey::new("42", SettlementWorld::Main, "100").expect("原信键");
    let observation = Uuid::from_u128(3);
    let quarantine = fake.seed_quarantine(key, QuarantineReason::ReturnUnproven, observation);
    let first = run(service.release_quarantine_admin(ReleaseQuarantineAdminInput {
        admin_actor: "admin-1".to_owned(),
        command_id: client_id(10),
        quarantine_id: quarantine,
        expected_version: 1,
        observation_id: observation,
        proposed_kind: ReleaseKind::AuctionReturnCase,
        request_id: None,
    }))
    .expect("交还原案");
    assert_eq!(first.decision_kind, ReleaseKind::AuctionReturnCase);
    assert_eq!(fake.counters().settlement_owners, 1);
    let conflict = run(service.release_quarantine_admin(ReleaseQuarantineAdminInput {
        admin_actor: "admin-1".to_owned(),
        command_id: client_id(11),
        quarantine_id: quarantine,
        expected_version: 1,
        observation_id: observation,
        proposed_kind: ReleaseKind::AuctionRecoveryReturn,
        request_id: None,
    }));
    assert_eq!(
        conflict,
        Err(ConsignmentError::Deposit(DepositBusinessRejection::DispositionConflict))
    );
    assert_eq!(fake.counters().recovery_owners, 0);

    let (fake, service) = setup();
    let quarantine = fake.seed_quarantine(
        OriginalMailKey::new("43", SettlementWorld::Main, "101").expect("原信键"),
        QuarantineReason::IdentityUnproven,
        observation,
    );
    fake.seed_existing_owner(quarantine, ReleaseKind::AuctionReturnCase);
    let conflict = run(service.release_quarantine_admin(ReleaseQuarantineAdminInput {
        admin_actor: "admin-1".to_owned(),
        command_id: client_id(12),
        quarantine_id: quarantine,
        expected_version: 1,
        observation_id: observation,
        proposed_kind: ReleaseKind::AuctionRecoveryReturn,
        request_id: None,
    }));
    assert!(conflict.is_err());
    assert_eq!(fake.counters().settlement_owners, 0);
    assert_eq!(fake.counters().recovery_owners, 0);
}

#[test]
fn lost_response_and_failed_commit_keep_the_ledger_consistent() {
    let (fake, service) = setup();
    seed_awaiting(&fake, Uuid::from_u128(LISTING));
    fake.lose_next_response();
    let lost = run(service.issue_reservation(
        seller(),
        client_id(20),
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ));
    assert_eq!(lost, Err(ConsignmentError::OutcomeUnknown));
    let first = run(service.issue_reservation(
        seller(),
        client_id(20),
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ))
    .expect("响应丢失后重试");
    assert_eq!(fake.snapshot().reservations.len(), 1);

    let (fake, service) = setup();
    seed_awaiting(&fake, Uuid::from_u128(LISTING));
    fake.fail_next_commit();
    let failed = run(service.issue_reservation(
        seller(),
        client_id(21),
        Uuid::from_u128(LISTING),
        version(1),
        None,
    ));
    assert_eq!(failed, Err(ConsignmentError::Unavailable));
    assert!(fake.snapshot().reservations.is_empty());
    assert!(!fake.snapshot().ledger.contains_key(&client_id(21)));
    let _ = first;
}

#[test]
fn staged_rollback_restores_the_whole_state() {
    let (fake, _service) = setup();
    fake.begin_stage();
    seed_awaiting(&fake, Uuid::from_u128(LISTING));
    assert!(!fake.snapshot().listings.is_empty());
    fake.rollback_stage();
    assert!(fake.snapshot().listings.is_empty());
}

#[test]
fn identity_allowlist_and_two_seasons_are_replaceable() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    seed_awaiting(&fake, listing_id);
    fake.set_authenticated(Some("43"), SettlementWorld::Isekai, 2);
    let clock = Timestamp::from_ymd_hms(2026, 11, 1, 12, 0, 0).expect("时钟");
    fake.set_clock(clock);
    fake.set_seasons(Some(season(clock)), Some(season(clock)));
    let at_cutoff =
        run(service.issue_reservation(seller(), client_id(30), listing_id, version(1), None));
    assert_eq!(at_cutoff, Err(ConsignmentError::Deposit(DepositBusinessRejection::SeasonClosed)));
    fake.advance_clock(auction_time::Duration::nanoseconds(-1));
    run(service.issue_reservation(seller(), client_id(31), listing_id, version(1), None))
        .expect("截止前可签发");
    fake.set_allowlist(false);
    let unavailable =
        run(service.issue_reservation(seller(), client_id(32), listing_id, version(2), None));
    assert_eq!(
        unavailable,
        Err(ConsignmentError::Deposit(DepositBusinessRejection::MailUnavailable))
    );
    fake.set_allowlist(true);
    fake.set_bot_party(true);
    let bot = run(service.issue_reservation(seller(), client_id(33), listing_id, version(2), None));
    assert_eq!(bot, Err(ConsignmentError::Deposit(DepositBusinessRejection::BotParty)));
    let snapshot = fake.snapshot();
    assert_eq!(snapshot.authenticated_uid.as_deref(), Some("43"));
    assert_eq!(snapshot.credential_generation, 2);
    assert_eq!(
        snapshot.frozen_season.as_ref().map(|season| season.season_id.as_str()),
        Some("2026-11-01")
    );
}

#[test]
fn controls_reads_proofs_and_seasons_use_the_same_ledger() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    fake.seed_listing(listing_id, seller(), "42", ListingState::Available, 3, 101);
    let withdrawn =
        run(service.request_withdrawal(seller(), client_id(40), listing_id, version(3), None))
            .expect("登记意愿");
    assert_eq!(withdrawn.state, ListingState::WithdrawalPending);

    let held_listing = Uuid::from_u128(2);
    fake.seed_listing(held_listing, seller(), "43", ListingState::Available, 1, 101);
    let hold = run(service.set_operations_hold(
        "admin-1".to_owned(),
        client_id(41),
        held_listing,
        version(1),
        crate::ports::consignment::ListingDelistReason::new("hold".to_owned()).expect("说明"),
        None,
    ))
    .expect("设置止售");
    let released = run(service.release_operations_hold(
        "admin-1".to_owned(),
        client_id(42),
        held_listing,
        version(2),
        hold.hold_id.expect("止售 ID"),
        None,
    ))
    .expect("解除止售");
    assert_eq!(
        released.held_reason,
        Some(auction_domain::consignment::HeldOffMarketReason::Review)
    );

    // 非 AwaitingDeposit 挂单即使残留活动预约也不能给出寄件指引：生产与替身都返回
    // `Some(OwnedDepositInstructions::None)`（本人可见但当前无指引）。
    let (_reservation, _token) = fake.seed_reservation(
        held_listing,
        seller(),
        "43",
        Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
    );
    assert_eq!(
        run(fake.instructions(seller(), held_listing)).expect("读取指引"),
        Some(OwnedDepositInstructions::None)
    );

    let deposit_listing = Uuid::from_u128(3);
    seed_awaiting(&fake, deposit_listing);
    let (_reservation, token) = fake.seed_reservation(
        deposit_listing,
        seller(),
        "44",
        Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
    );
    let instructions =
        run(fake.instructions(seller(), deposit_listing)).expect("读取指引").expect("本人");
    let OwnedDepositInstructions::Active(active) = instructions else {
        panic!("应有有效指引");
    };
    assert_eq!(active.body, token);

    let status = run(fake.status()).expect("状态");
    assert_eq!(status.fee_bps, FeeBps::DEFAULT_CONSIGNED);
    assert!(!status.purchasable);
    assert_eq!(status.worlds.main.season_state, SeasonState::NotSeasonal);

    let key = OriginalMailKey::new("42", SettlementWorld::Main, "200").expect("原信键");
    let observation = Uuid::from_u128(30);
    let quarantine = fake.seed_quarantine(key, QuarantineReason::ReturnUnproven, observation);
    let detail = run(fake.quarantine(AdminDepositQuery {
        quarantine_id: quarantine,
        cursor: None,
        limit: 10,
    }))
    .expect("查询隔离")
    .expect("存在");
    assert_eq!(
        detail.allowed_actions,
        vec![AdminDepositAction::Recheck, AdminDepositAction::ReleaseQuarantine]
    );
    assert_eq!(detail.observation_total, 1);
    assert_eq!(detail.observations[0].returnable, Some(true));

    fake.set_next_proof_disposition(ProofDisposition::TakeProven);
    let proof = run(service.apply_mail_proof(ApplyMailProofInput {
        original_attempt_id: Uuid::from_u128(50),
        proof_kind: MailProofKind::ExactTake,
        final_decision_id: Uuid::from_u128(51),
        action: DispositionAction::Take,
        delivery_id: Uuid::from_u128(52),
        fence: 1,
        observation_id: Uuid::from_u128(53),
        validator_version: "v1".to_owned(),
        proof_digest: [0x44; 32],
    }))
    .expect("应用证明");
    assert_eq!(proof.disposition.as_str(), ProofDisposition::TakeProven.as_str());

    let season = season(Timestamp::UNIX_EPOCH + auction_time::Duration::hours(2));
    let notice = run(service.queue_season_notice(
        season.clone(),
        Uuid::from_u128(60),
        seller(),
        SeasonNoticeKind::PrecloseWithdrawal,
    ))
    .expect("通知");
    assert_eq!(notice.disposition.as_str(), "notice_queued");
    fake.set_clock(season.end_utc);
    let closed = run(service
        .close_season(season, crate::ports::consignment::SeasonCloseTarget::Listing(held_listing)))
    .expect("关闭季");
    assert_eq!(closed.closed_listing_ids, vec![held_listing]);
    assert_eq!(
        fake.snapshot().listings.get(&held_listing).map(|listing| listing.state),
        Some(ListingState::Closed)
    );
    assert_eq!(
        fake.snapshot().listings.get(&held_listing).and_then(|listing| listing.reason),
        Some(auction_domain::consignment::ListingReason::SeasonClosed)
    );
}

/// 不变量 2、3、6：v2 Admin 下架在锁内按状态分流；已提交命令的首次回执不因后续状态变化而改写。
#[test]
fn admin_delist_closes_unmanaged_holds_managed_and_replays_the_first_receipt() {
    let (fake, service) = setup();
    let reason =
        || crate::ports::consignment::ListingDelistReason::new("hold".to_owned()).expect("说明");

    let unmanaged = Uuid::from_u128(10);
    seed_awaiting(&fake, unmanaged);
    let (_, _token) = fake.seed_reservation(
        unmanaged,
        seller(),
        "42",
        Timestamp::UNIX_EPOCH + auction_time::Duration::hours(1),
    );
    let first = run(service.set_operations_hold(
        "admin-1".to_owned(),
        client_id(90),
        unmanaged,
        version(1),
        reason(),
        None,
    ))
    .expect("未代管件关闭");
    assert_eq!(first.disposition, crate::ports::consignment::ReceiptDisposition::Closed);
    assert_eq!(first.hold_id, None);
    assert_eq!(first.state, ListingState::Closed);
    let snapshot = fake.snapshot();
    let listing = snapshot.listings.get(&unmanaged).expect("挂单");
    assert_eq!(listing.state, ListingState::Closed);
    assert_eq!(listing.reason, Some(auction_domain::consignment::ListingReason::AdminDelisted));
    assert_eq!(listing.version, 2);
    assert!(snapshot.reservations.values().all(|reservation| {
        reservation.listing_id != unmanaged || reservation.state == ReservationState::Retired
    }));
    let replay = run(service.set_operations_hold(
        "admin-1".to_owned(),
        client_id(90),
        unmanaged,
        version(1),
        reason(),
        None,
    ))
    .expect("同命令重放");
    assert_eq!(replay, first);

    for (offset, state) in
        [ListingState::DepositAccepted, ListingState::Available, ListingState::HeldOffMarket]
            .into_iter()
            .enumerate()
    {
        let listing_id = Uuid::from_u128(20 + offset as u128);
        fake.seed_listing(listing_id, seller(), "50", state, 1, 100);
        let hold = run(service.set_operations_hold(
            "admin-1".to_owned(),
            client_id(91 + offset as u128),
            listing_id,
            version(1),
            reason(),
            None,
        ))
        .expect("代管件设止售");
        assert_eq!(hold.disposition, crate::ports::consignment::ReceiptDisposition::HoldSet);
        assert!(hold.hold_id.is_some());
        assert_eq!(hold.state, ListingState::HeldOffMarket);
    }

    for (offset, state) in
        [ListingState::SalePending, ListingState::WithdrawalPending].into_iter().enumerate()
    {
        let listing_id = Uuid::from_u128(30 + offset as u128);
        fake.seed_listing(listing_id, seller(), "60", state, 1, 100);
        assert_eq!(
            run(service.set_operations_hold(
                "admin-1".to_owned(),
                client_id(95 + offset as u128),
                listing_id,
                version(1),
                reason(),
                None,
            )),
            Err(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))
        );
    }
}

#[test]
fn expiry_and_unmanaged_withdrawal_follow_phase_four_edges() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    fake.seed_listing(listing_id, seller(), "42", ListingState::AwaitingDeposit, 1, 101);
    let issued =
        run(service.issue_reservation(seller(), client_id(70), listing_id, version(1), None))
            .expect("签发");
    fake.set_clock(issued.expires_at);
    let retired = run(<FakeDepositStore as ReservationStore>::expire_reservation(
        &fake,
        crate::ports::consignment::ExpireReservationCommand {
            command_id: CommandId::from_uuid(Uuid::from_u128(71)),
            reservation_id: issued.reservation_id,
            generation: 1,
            invalidation: crate::ports::consignment::ReservationInvalidation::Expired {
                expires_at: issued.expires_at,
            },
            request_hash: crate::ports::core::RequestHash::new([1; 32]),
        },
    ))
    .expect("到期退休");
    assert_eq!(retired.reservation_id, issued.reservation_id);

    let unmanaged = Uuid::from_u128(3);
    fake.seed_listing(unmanaged, seller(), "44", ListingState::Displayed, 1, 101);
    let closed =
        run(service.request_withdrawal(seller(), client_id(72), unmanaged, version(1), None))
            .expect("未代管关闭");
    assert_eq!(
        closed.disposition.as_str(),
        crate::ports::consignment::WithdrawDisposition::Closed.as_str()
    );
    assert_eq!(closed.state, ListingState::Closed);
    let snapshot = fake.snapshot();
    assert_eq!(
        snapshot.listings.get(&unmanaged).and_then(|listing| listing.reason),
        Some(auction_domain::consignment::ListingReason::SellerCancelled)
    );

    // 直接调用 EnvelopeStore 以覆盖写入端口的同 ID 重放分支。
    let key = OriginalMailKey::new("42", SettlementWorld::Main, "300").expect("原信键");
    let quarantine =
        fake.seed_quarantine(key, QuarantineReason::ReturnUnproven, Uuid::from_u128(80));
    let command = ReleaseQuarantineCommand {
        command_id: client_id(73),
        actor: crate::ports::consignment::DepositActor::Admin("admin-1".to_owned()),
        quarantine_id: quarantine,
        expected_version: 1,
        observation_id: Uuid::from_u128(80),
        proposed_kind: ReleaseKind::Return,
        request_hash: crate::ports::core::RequestHash::new([2; 32]),
        request_id: None,
        auto_binding: None,
    };
    let first =
        run(<FakeDepositStore as EnvelopeStore>::release_quarantine(&fake, command.clone()))
            .expect("首次解除");
    let replay = run(<FakeDepositStore as EnvelopeStore>::release_quarantine(&fake, command))
        .expect("同 ID 重放");
    assert_eq!(first, replay);

    // 起写回执构造器与许可访问器。
    let receipt = StartReceipt::started(
        client_id(74),
        Uuid::from_u128(81),
        Uuid::from_u128(82),
        Uuid::from_u128(83),
        4,
    );
    assert_eq!(receipt.disposition.as_str(), "write_started");
}

#[test]
fn admin_deposit_actions_use_the_wire_names_from_the_contract() {
    assert_eq!(AdminDepositAction::Recheck.as_str(), "recheck");
    assert_eq!(AdminDepositAction::ReleaseQuarantine.as_str(), "release");
}

#[test]
fn phase_four_expiry_and_season_boundaries_are_exact_and_notices_deduplicate() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    seed_awaiting(&fake, listing_id);
    let issued =
        run(service.issue_reservation(seller(), client_id(100), listing_id, version(1), None))
            .expect("签发");
    fake.set_clock(
        issued
            .expires_at
            .checked_add_signed(auction_time::Duration::nanoseconds(-1))
            .expect("前一微秒"),
    );
    assert_eq!(
        run(service.expire_reservation(
            issued.reservation_id,
            1,
            crate::ports::consignment::ReservationInvalidation::Expired {
                expires_at: issued.expires_at,
            },
        )),
        Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))
    );
    fake.set_clock(issued.expires_at);
    run(service.expire_reservation(
        issued.reservation_id,
        1,
        crate::ports::consignment::ReservationInvalidation::Expired {
            expires_at: issued.expires_at,
        },
    ))
    .expect("截止等号退休");

    let close_listing = Uuid::from_u128(2);
    seed_awaiting(&fake, close_listing);
    let season = season(Timestamp::UNIX_EPOCH + auction_time::Duration::hours(2));
    fake.set_clock(
        season
            .end_utc
            .checked_add_signed(auction_time::Duration::nanoseconds(-1))
            .expect("前一微秒"),
    );
    assert_eq!(
        run(service.close_season(
            season.clone(),
            crate::ports::consignment::SeasonCloseTarget::Listing(close_listing),
        )),
        Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))
    );
    fake.set_clock(season.end_utc);
    run(service.close_season(
        season.clone(),
        crate::ports::consignment::SeasonCloseTarget::Listing(close_listing),
    ))
    .expect("季末等号关闭");

    let position = Uuid::from_u128(60);
    let first = run(service.queue_season_notice(
        season.clone(),
        position,
        seller(),
        SeasonNoticeKind::PrecloseWithdrawal,
    ))
    .expect("首次通知");
    let second = run(service.queue_season_notice(
        season,
        position,
        seller(),
        SeasonNoticeKind::PrecloseWithdrawal,
    ))
    .expect("不同命令的同一通知");
    assert_eq!(second.work_id, first.work_id);
    assert_eq!(fake.snapshot().notices.len(), 1);
}

#[test]
fn fake_expire_reservation_rejects_cancelled_invalidation() {
    let (fake, service) = setup();
    let listing_id = Uuid::from_u128(LISTING);
    seed_awaiting(&fake, listing_id);
    let issued =
        run(service.issue_reservation(seller(), client_id(110), listing_id, version(1), None))
            .expect("签发");
    let error = run(service.expire_reservation(
        issued.reservation_id,
        1,
        crate::ports::consignment::ReservationInvalidation::Cancelled {
            command_id: client_id(111),
        },
    ))
    .expect_err("取消不能单独退休");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed));
    let snapshot = fake.snapshot();
    assert_eq!(
        snapshot.reservations.get(&issued.reservation_id).map(|reservation| reservation.state),
        Some(ReservationState::Active)
    );
}
