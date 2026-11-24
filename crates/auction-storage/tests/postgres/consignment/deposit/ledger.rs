//! v2 账本仲裁、类型化目标与 4A 回执逐类型往返。

use auction_application::consignment::{
    ConsignmentError, CustodyListingChange, DepositActor, DepositOperation, EnvelopeDisposition,
    EnvelopeReceipt, ObservationReceipt, OperationsHoldDisposition, OperationsHoldReceipt,
    ProofDisposition, ProofReceipt, QuarantineReference, RecheckReceipt, ReleaseReceipt,
    ReservationReceipt, ReservationRetired, SeasonCloseReceipt, SeasonCloseTarget,
    SeasonNoticeReceipt, StartReceipt, WithdrawDepositReceipt, WithdrawDisposition,
};
use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{PositionState, ReleaseKind};
use auction_domain::consignment::{
    HeldOffMarketReason, ListingEquipmentIdentity, ListingState, ListingVersion, OriginalMailKey,
};
use auction_storage::{
    TestDepositLocks, TestDepositOutcome, TestDepositReceipt, TestDepositTarget,
    test_deposit_audit_count, test_deposit_command_exists, test_deposit_event_count,
};
use uuid::Uuid;

use super::support::{LISTING, USER_1, USER_2, execute, listing_awaiting_deposit, users_and_key};
use crate::support::TestDatabase;

const COMMAND_BASE: u128 = 0x4b00_0000_0000_4000_8000_0000_0000_0000;
const THIRD_LISTING: &str = "21000000-0000-0000-0000-000000000003";
const THIRD_SNAPSHOT: &str = "31000000-0000-0000-0000-000000000003";
const THIRD_COMMAND: &str = "41000000-0000-5000-8000-000000000093";
const CLOSABLE_LISTING: &str = "21000000-0000-0000-0000-000000000004";
const CLOSABLE_SNAPSHOT: &str = "31000000-0000-0000-0000-000000000004";
const CLOSABLE_REGISTER: &str = "41000000-0000-4000-8000-000000000095";
const CLOSABLE_PUBLISH: &str = "41000000-0000-5000-8000-000000000095";

fn uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn command(n: u128) -> Uuid {
    uuid(COMMAND_BASE + n)
}

fn worker_command(n: u128) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("deposit-worker-{n}").as_bytes())
}

fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("挂单 UUID")
}

fn seller_id() -> Uuid {
    Uuid::parse_str(USER_1).expect("卖家 UUID")
}

fn mail_key() -> OriginalMailKey {
    OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键")
}

fn locks(command_id: Uuid) -> TestDepositLocks {
    TestDepositLocks {
        command_id,
        sellers: vec![seller_id()],
        equipment: vec![ListingEquipmentIdentity::new("main", "42").expect("装备身份")],
        mail_keys: vec![mail_key()],
    }
}

fn worker() -> DepositActor {
    DepositActor::Worker
}

fn public_user() -> DepositActor {
    DepositActor::PublicUser(UserId::from_uuid(seller_id()))
}

fn admin() -> DepositActor {
    DepositActor::Admin("fake-admin".into())
}

async fn seed() -> TestDatabase {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接运行账号");
    execute(store.postgres_pool(), &users_and_key()).await.expect("写入用户");
    execute(store.postgres_pool(), &listing_awaiting_deposit()).await.expect("写入挂单");
    execute(store.postgres_pool(), &third_listing_accepted()).await.expect("写入已接受挂单");
    execute(store.postgres_pool(), &closable_listing()).await.expect("写入可关闭挂单");
    database
}

fn closable_listing() -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{CLOSABLE_LISTING}', '{USER_1}', 'main', '45', 'consigned', 'resolving', 101,
                 1, 500, 1, 1, 0, ARRAY[]::uuid[], '2026-01-01T00:00:00Z',
                 '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{CLOSABLE_SNAPSHOT}', '{CLOSABLE_LISTING}', 1, 'hvdata-equip-v1',
                 '2026-01-01T01:00:00Z', decode(repeat('99', 32), 'hex'), 'main', '45',
                 'Fake Gear Five', 'Weapon', NULL, 'assigned', 100, true, 'FakeSellerOne',
                 '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{CLOSABLE_REGISTER}', 'public_user', '{USER_1}', 'register', 'new',
                 decode(repeat('9a', 32), 'hex'), '90000000-0000-0000-0000-000000000001',
                 '{CLOSABLE_LISTING}', 202, 'accepted',
                 '{{\"command_id\":\"{CLOSABLE_REGISTER}\",\"operation\":\"register\",
                    \"listing_id\":\"{CLOSABLE_LISTING}\",\"listing_version\":1,
                    \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                 '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{CLOSABLE_LISTING}', 1, '{CLOSABLE_REGISTER}', 'public_user', 'registered',
                 NULL, 'resolving', NULL, 101, '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{CLOSABLE_PUBLISH}', 'worker', 'fake-worker', 'apply_lookup',
                 '{CLOSABLE_LISTING}', decode(repeat('9b', 32), 'hex'), NULL,
                 '{CLOSABLE_LISTING}', 200, 'ok',
                 '{{\"command_id\":\"{CLOSABLE_PUBLISH}\",\"operation\":\"apply_lookup\",
                    \"listing_id\":\"{CLOSABLE_LISTING}\",\"listing_version\":2,
                    \"state\":\"awaiting_deposit\",\"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"2026-01-02T00:00:00Z\",\"reason_code\":null}}',
                 '2026-01-01T01:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{CLOSABLE_LISTING}', 2, '{CLOSABLE_PUBLISH}', 'worker', 'published',
                 'resolving', 'awaiting_deposit', NULL, 101, '2026-01-01T01:00:00Z');
         UPDATE public.consignment_listings
         SET state='awaiting_deposit', state_version=2,
             current_snapshot_id='{CLOSABLE_SNAPSHOT}', updated_at='2026-01-01T01:00:00Z'
         WHERE id='{CLOSABLE_LISTING}';"
    )
}

fn third_listing_accepted() -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{THIRD_LISTING}', '{USER_1}', 'main', '44', 'consigned',
                 'resolving', 101, 1, 500, 1, 1, 0, ARRAY[]::uuid[],
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{THIRD_SNAPSHOT}', '{THIRD_LISTING}', 1, 'hvdata-equip-v1',
                 '2026-01-01T01:00:00Z', decode(repeat('95', 32), 'hex'), 'main', '44',
                 'Fake Gear Three', 'Weapon', NULL, 'assigned', 100, true, 'FakeSellerOne',
                 '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('41000000-0000-4000-8000-000000000094', 'public_user', '{USER_1}',
                 'register', 'new', decode(repeat('97', 32), 'hex'),
                 '90000000-0000-0000-0000-000000000001', '{THIRD_LISTING}', 202, 'accepted',
                 '{{\"command_id\":\"41000000-0000-4000-8000-000000000094\",
                    \"operation\":\"register\",\"listing_id\":\"{THIRD_LISTING}\",
                    \"listing_version\":1,\"state\":\"resolving\",
                    \"disposition\":\"registered\",\"price\":101}}',
                 '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{THIRD_LISTING}', 1, '41000000-0000-4000-8000-000000000094',
                 'public_user', 'registered', NULL, 'resolving', NULL, 101,
                 '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('41000000-0000-5000-8000-000000000094', 'worker', 'fake-worker',
                 'apply_lookup', '{THIRD_LISTING}', decode(repeat('98', 32), 'hex'),
                 NULL, '{THIRD_LISTING}', 200, 'ok',
                 '{{\"command_id\":\"41000000-0000-5000-8000-000000000094\",
                    \"operation\":\"apply_lookup\",\"listing_id\":\"{THIRD_LISTING}\",
                    \"listing_version\":2,\"state\":\"awaiting_deposit\",
                    \"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"2026-01-02T00:00:00Z\",\"reason_code\":null}}',
                 '2026-01-01T01:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{THIRD_LISTING}', 2, '41000000-0000-5000-8000-000000000094',
                 'worker', 'published', 'resolving', 'awaiting_deposit', NULL, 101,
                 '2026-01-01T01:00:00Z');
         UPDATE public.consignment_listings
         SET state='awaiting_deposit', state_version=2, current_snapshot_id='{THIRD_SNAPSHOT}',
             updated_at='2026-01-01T01:00:00Z'
         WHERE id='{THIRD_LISTING}';
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_bot_uid, target_world_id, target_external_mid,
             result_status, result_code, result_body, created_at)
         VALUES ('{THIRD_COMMAND}', 2, 'worker', 'fake-worker', 'decide_envelope',
                 '12345|main|777', decode(repeat('96', 32), 'hex'), 'mail_key', '12345',
                 'main', '777', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"decide_envelope\",
                    \"disposition\":\"accepted\",\"command_id\":\"{THIRD_COMMAND}\",
                    \"decision_id\":\"41000000-0000-4000-8000-000000000093\",
                    \"listing_changes\":[{{\"listing_id\":\"{THIRD_LISTING}\",
                    \"listing_version\":3,\"state\":\"deposit_accepted\"}}],
                    \"quarantine\":null}}', '2026-01-01T01:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{THIRD_LISTING}', 3, '{THIRD_COMMAND}', 'worker', 'deposit_accepted',
                 'awaiting_deposit', 'deposit_accepted', NULL, 101, '2026-01-01T01:00:00Z');
         UPDATE public.consignment_listings
         SET state='deposit_accepted', state_version=3, updated_at='2026-01-01T01:00:00Z'
         WHERE id='{THIRD_LISTING}';"
    )
}

/// 每种目标类型的同 ID 同请求都返回原回执，异摘要只回冲突。
#[tokio::test]
async fn postgres_deposit_v2_ledger_replays_each_typed_target() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let cases = vec![
        (
            DepositOperation::IssueReservation,
            public_user(),
            TestDepositTarget::Listing(listing_id()),
            TestDepositReceipt::Reservation(ReservationReceipt::issued(
                CommandId::from_uuid(command(1)),
                listing_id(),
                ListingVersion::new(3).expect("版本"),
                ListingState::AwaitingDeposit,
                uuid(0x100),
                auction_time::Timestamp::parse_from_rfc3339("2026-01-01T12:00:00Z").expect("时刻"),
            )),
        ),
        (
            DepositOperation::ExpireReservation,
            worker(),
            TestDepositTarget::Reservation(uuid(0x200)),
            TestDepositReceipt::ReservationRetired(ReservationRetired::new(
                CommandId::from_uuid(worker_command(2)),
                uuid(0x200),
            )),
        ),
        (
            DepositOperation::RecheckQuarantine,
            admin(),
            TestDepositTarget::Quarantine(uuid(0x300)),
            TestDepositReceipt::Recheck(RecheckReceipt::queued(
                CommandId::from_uuid(command(3)),
                uuid(0x300),
                uuid(0x301),
            )),
        ),
        (
            DepositOperation::StartDispositionWrite,
            worker(),
            TestDepositTarget::FinalDecision(uuid(0x400)),
            TestDepositReceipt::Start(StartReceipt::started(
                CommandId::from_uuid(worker_command(4)),
                uuid(0x400),
                uuid(0x401),
                uuid(0x402),
                1,
            )),
        ),
        (
            DepositOperation::ApplyMailProof,
            worker(),
            TestDepositTarget::OriginalAttempt(uuid(0x500)),
            TestDepositReceipt::Proof(ProofReceipt::new(
                CommandId::from_uuid(worker_command(5)),
                ProofDisposition::EvidenceQuarantined,
                uuid(0x501),
                uuid(0x502),
            )),
        ),
        (
            DepositOperation::DecideEnvelope,
            worker(),
            TestDepositTarget::MailKey(mail_key()),
            TestDepositReceipt::Envelope({
                let mut receipt = EnvelopeReceipt::new(
                    CommandId::from_uuid(worker_command(6)),
                    EnvelopeDisposition::Quarantined,
                    uuid(0x601),
                );
                receipt.quarantine =
                    Some(QuarantineReference { quarantine_id: uuid(0x602), version: 1 });
                receipt
            }),
        ),
        (
            DepositOperation::SeasonClose,
            worker(),
            TestDepositTarget::Season { season_id: "2026-11".into(), calendar_version: [0x11; 32] },
            TestDepositReceipt::SeasonClose(SeasonCloseReceipt::closed(
                CommandId::from_uuid(worker_command(7)),
                "2026-11".into(),
                SeasonCloseTarget::World(SettlementWorld::Main),
                vec![listing_id()],
            )),
        ),
    ];

    for (index, (operation, actor, target, receipt)) in cases.into_iter().enumerate() {
        let command_id = receipt.command_id().as_uuid();
        let conflict_actor = actor.clone();
        let conflict_target = target.clone();
        let conflict_receipt = receipt.clone();
        let first = store
            .test_deposit_submit_receipt(
                locks(command_id),
                actor.clone(),
                operation,
                target.clone(),
                [index as u8 + 1; 32],
                None,
                receipt.clone(),
            )
            .await
            .expect("首次写入");
        let first = match first {
            TestDepositOutcome::First(stored) => stored,
            TestDepositOutcome::Replay(_) => panic!("首次不应重放"),
        };
        let replay = store
            .test_deposit_submit_receipt(
                locks(command_id),
                actor,
                operation,
                target,
                [index as u8 + 1; 32],
                None,
                receipt,
            )
            .await
            .expect("同请求重放");
        let TestDepositOutcome::Replay(replayed) = replay else {
            panic!("同请求应重放");
        };
        assert_eq!(first.status, replayed.status);
        assert_eq!(
            serde_json::to_vec(&first.body).expect("首次回执 JSON"),
            serde_json::to_vec(&replayed.body).expect("重放回执 JSON")
        );
        assert_eq!(
            store
                .test_deposit_submit_receipt(
                    locks(command_id),
                    conflict_actor,
                    operation,
                    conflict_target,
                    [0xff; 32],
                    None,
                    conflict_receipt,
                )
                .await,
            Err(ConsignmentError::CommandIdConflict)
        );
    }
}

/// 同一命令 ID 的 actor、operation、target 或摘要任一不同都必须拒绝，不能写第二份事实。
#[tokio::test]
async fn postgres_deposit_v2_ledger_conflicts_on_every_identity_axis() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let command_id = command(0x20);
    let digest = [0x20; 32];
    let receipt = ReservationReceipt::issued(
        CommandId::from_uuid(command_id),
        listing_id(),
        ListingVersion::new(3).expect("版本"),
        ListingState::AwaitingDeposit,
        uuid(0x201),
        auction_time::Timestamp::parse_from_rfc3339("2026-01-01T12:00:00Z").expect("时刻"),
    );
    store
        .test_deposit_submit_receipt(
            locks(command_id),
            public_user(),
            DepositOperation::IssueReservation,
            TestDepositTarget::Listing(listing_id()),
            digest,
            None,
            TestDepositReceipt::Reservation(receipt.clone()),
        )
        .await
        .expect("首次写入");

    let conflict = |actor, operation, target, digest, receipt| {
        store.test_deposit_submit_receipt(
            locks(command_id),
            actor,
            operation,
            target,
            digest,
            None,
            receipt,
        )
    };
    assert_eq!(
        conflict(
            admin(),
            DepositOperation::IssueReservation,
            TestDepositTarget::Listing(listing_id()),
            digest,
            TestDepositReceipt::Reservation(receipt.clone()),
        )
        .await,
        Err(ConsignmentError::CommandIdConflict),
        "actor 不同必须冲突"
    );
    assert_eq!(
        conflict(
            DepositActor::PublicUser(UserId::from_uuid(
                Uuid::parse_str(USER_2).expect("第二用户 UUID"),
            )),
            DepositOperation::IssueReservation,
            TestDepositTarget::Listing(listing_id()),
            digest,
            TestDepositReceipt::Reservation(receipt.clone()),
        )
        .await,
        Err(ConsignmentError::CommandIdConflict),
        "同为 public_user 但用户不同必须冲突"
    );
    assert_eq!(
        conflict(
            public_user(),
            DepositOperation::Withdraw,
            TestDepositTarget::Listing(listing_id()),
            digest,
            TestDepositReceipt::Withdraw(WithdrawDepositReceipt::new(
                CommandId::from_uuid(command_id),
                WithdrawDisposition::Closed,
                listing_id(),
                ListingVersion::new(3).expect("版本"),
                ListingState::Closed,
                None,
            )),
        )
        .await,
        Err(ConsignmentError::CommandIdConflict),
        "operation 不同必须冲突"
    );
    assert_eq!(
        conflict(
            public_user(),
            DepositOperation::IssueReservation,
            TestDepositTarget::Listing(uuid(0x202)),
            digest,
            TestDepositReceipt::Reservation(receipt.clone()),
        )
        .await,
        Err(ConsignmentError::CommandIdConflict),
        "target 不同必须冲突"
    );
    assert_eq!(
        conflict(
            public_user(),
            DepositOperation::IssueReservation,
            TestDepositTarget::Listing(listing_id()),
            [0x21; 32],
            TestDepositReceipt::Reservation(receipt),
        )
        .await,
        Err(ConsignmentError::CommandIdConflict),
        "摘要不同必须冲突"
    );
    assert!(test_deposit_command_exists(&store, command_id).await);
    assert_eq!(test_deposit_event_count(&store, command_id).await.expect("事件数"), 1);
    assert_eq!(test_deposit_audit_count(&store, command_id).await.expect("审计数"), 1);
}

/// 同为 Admin 但引用不同、Worker 与 System 来源不同都必须冲突且不写第二条事实。
#[tokio::test]
async fn postgres_deposit_v2_ledger_conflicts_when_actor_references_differ() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");

    let admin_command = command(0x21);
    let admin_receipt =
        || RecheckReceipt::queued(CommandId::from_uuid(admin_command), uuid(0x211), uuid(0x212));
    store
        .test_deposit_submit_receipt(
            locks(admin_command),
            admin(),
            DepositOperation::RecheckQuarantine,
            TestDepositTarget::Quarantine(uuid(0x211)),
            [0x22; 32],
            None,
            TestDepositReceipt::Recheck(admin_receipt()),
        )
        .await
        .expect("Admin 首次写入");
    assert_eq!(
        store
            .test_deposit_submit_receipt(
                locks(admin_command),
                DepositActor::Admin("other-admin".into()),
                DepositOperation::RecheckQuarantine,
                TestDepositTarget::Quarantine(uuid(0x211)),
                [0x22; 32],
                None,
                TestDepositReceipt::Recheck(admin_receipt()),
            )
            .await,
        Err(ConsignmentError::CommandIdConflict),
        "同为 admin 但引用不同必须冲突"
    );
    assert_eq!(test_deposit_event_count(&store, admin_command).await.expect("事件数"), 0);
    assert_eq!(test_deposit_audit_count(&store, admin_command).await.expect("审计数"), 1);

    let system_command = worker_command(0x22);
    let release_receipt = || {
        ReleaseReceipt::released(
            CommandId::from_uuid(system_command),
            uuid(0x221),
            uuid(0x222),
            uuid(0x223),
            ReleaseKind::Return,
        )
    };
    store
        .test_deposit_submit_receipt(
            locks(system_command),
            DepositActor::System,
            DepositOperation::ReleaseQuarantine,
            TestDepositTarget::Quarantine(uuid(0x221)),
            [0x23; 32],
            None,
            TestDepositReceipt::Release(release_receipt()),
        )
        .await
        .expect("System 首次写入");
    assert_eq!(
        store
            .test_deposit_submit_receipt(
                locks(system_command),
                DepositActor::Worker,
                DepositOperation::ReleaseQuarantine,
                TestDepositTarget::Quarantine(uuid(0x221)),
                [0x23; 32],
                None,
                TestDepositReceipt::Release(release_receipt()),
            )
            .await,
        Err(ConsignmentError::CommandIdConflict),
        "worker 与 system 来源不同必须冲突"
    );
    assert_eq!(test_deposit_event_count(&store, system_command).await.expect("事件数"), 0);
    assert_eq!(test_deposit_audit_count(&store, system_command).await.expect("审计数"), 1);
}

/// v1 与 v2 共用全局主键；v1 ID 不能另发 v2 请求。
#[tokio::test]
async fn postgres_deposit_v2_ledger_shares_the_v1_primary_key() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    let v1_command = Uuid::parse_str(super::support::REGISTER).expect("v1 命令 UUID");
    let error = store
        .test_deposit_submit_receipt(
            locks(v1_command),
            public_user(),
            DepositOperation::Withdraw,
            TestDepositTarget::Listing(listing_id()),
            [0x21; 32],
            None,
            TestDepositReceipt::Withdraw(WithdrawDepositReceipt::new(
                CommandId::from_uuid(v1_command),
                WithdrawDisposition::WithdrawalRequested,
                listing_id(),
                ListingVersion::new(3).expect("版本"),
                ListingState::DepositAccepted,
                Some(uuid(0x700)),
            )),
        )
        .await
        .expect_err("v1 ID 必须冲突");
    assert_eq!(error, ConsignmentError::CommandIdConflict);
}

fn receipt_cases() -> Vec<(TestDepositLocks, DepositActor, TestDepositTarget, TestDepositReceipt)> {
    let mut cases = Vec::new();
    let mut add =
        |_n: u128, actor: DepositActor, target: TestDepositTarget, receipt: TestDepositReceipt| {
            let command_id = receipt.command_id().as_uuid();
            cases.push((locks(command_id), actor, target, receipt));
        };
    add(
        0x30,
        public_user(),
        TestDepositTarget::Listing(listing_id()),
        TestDepositReceipt::Reservation(ReservationReceipt::issued(
            CommandId::from_uuid(command(0x30)),
            listing_id(),
            ListingVersion::new(3).expect("版本"),
            ListingState::AwaitingDeposit,
            uuid(0x301),
            auction_time::Timestamp::parse_from_rfc3339("2026-01-01T12:00:00Z").expect("时刻"),
        )),
    );
    add(
        0x31,
        worker(),
        TestDepositTarget::Reservation(uuid(0x310)),
        TestDepositReceipt::ReservationRetired(ReservationRetired::new(
            CommandId::from_uuid(worker_command(0x31)),
            uuid(0x310),
        )),
    );
    add(
        0x32,
        worker(),
        TestDepositTarget::MailKey(mail_key()),
        TestDepositReceipt::Envelope({
            let mut receipt = EnvelopeReceipt::new(
                CommandId::from_uuid(worker_command(0x32)),
                EnvelopeDisposition::Accepted,
                uuid(0x320),
            );
            receipt.listing_changes = vec![auction_application::consignment::ListingStateChange {
                listing_id: listing_id(),
                listing_version: 3,
                state: ListingState::DepositAccepted,
            }];
            receipt
        }),
    );
    add(
        0x33,
        worker(),
        TestDepositTarget::Quarantine(uuid(0x330)),
        TestDepositReceipt::Observation(ObservationReceipt::appended(
            CommandId::from_uuid(worker_command(0x33)),
            uuid(0x330),
            2,
            uuid(0x331),
            [0x33; 32],
        )),
    );
    add(
        0x34,
        admin(),
        TestDepositTarget::Quarantine(uuid(0x340)),
        TestDepositReceipt::Recheck(RecheckReceipt::queued(
            CommandId::from_uuid(command(0x34)),
            uuid(0x340),
            uuid(0x341),
        )),
    );
    add(
        0x35,
        admin(),
        TestDepositTarget::Quarantine(uuid(0x350)),
        TestDepositReceipt::Release({
            let mut receipt = auction_application::consignment::ReleaseReceipt::released(
                CommandId::from_uuid(command(0x35)),
                uuid(0x350),
                uuid(0x351),
                uuid(0x352),
                auction_domain::consignment::deposit::ReleaseKind::Accept,
            );
            receipt.listing_changes = vec![auction_application::consignment::ListingStateChange {
                listing_id: listing_id(),
                listing_version: 3,
                state: ListingState::DepositAccepted,
            }];
            receipt
        }),
    );
    add(
        0x36,
        worker(),
        TestDepositTarget::FinalDecision(uuid(0x360)),
        TestDepositReceipt::Start(StartReceipt::started(
            CommandId::from_uuid(worker_command(0x36)),
            uuid(0x360),
            uuid(0x361),
            uuid(0x362),
            7,
        )),
    );
    add(
        0x37,
        worker(),
        TestDepositTarget::OriginalAttempt(uuid(0x370)),
        TestDepositReceipt::Proof({
            let mut receipt = ProofReceipt::new(
                CommandId::from_uuid(worker_command(0x37)),
                ProofDisposition::TakeProven,
                uuid(0x371),
                uuid(0x372),
            );
            receipt.listing_changes = vec![auction_application::consignment::ListingStateChange {
                listing_id: listing_id(),
                listing_version: 3,
                state: ListingState::HeldOffMarket,
            }];
            receipt.custody_changes = vec![CustodyListingChange {
                listing_id: listing_id(),
                listing_version: 3,
                state: ListingState::HeldOffMarket,
                held_reason: Some(HeldOffMarketReason::OperationsHold),
                position_id: uuid(0x373),
                position_state: PositionState::InCustody,
            }];
            receipt
        }),
    );
    add(
        0x38,
        public_user(),
        TestDepositTarget::Listing(listing_id()),
        TestDepositReceipt::Withdraw(WithdrawDepositReceipt::new(
            CommandId::from_uuid(command(0x38)),
            WithdrawDisposition::Closed,
            listing_id(),
            ListingVersion::new(4).expect("版本"),
            ListingState::Closed,
            None,
        )),
    );
    add(
        0x39,
        admin(),
        TestDepositTarget::Listing(Uuid::parse_str(THIRD_LISTING).expect("第三挂单")),
        TestDepositReceipt::OperationsHold(
            OperationsHoldReceipt::new(
                CommandId::from_uuid(command(0x39)),
                OperationsHoldDisposition::HoldSet,
                Uuid::parse_str(THIRD_LISTING).expect("第三挂单"),
                ListingVersion::new(4).expect("版本"),
                ListingState::HeldOffMarket,
                Some(uuid(0x390)),
                Some(HeldOffMarketReason::OperationsHold),
            )
            .expect("止售设置回执"),
        ),
    );
    add(
        0x3a,
        admin(),
        TestDepositTarget::Listing(Uuid::parse_str(THIRD_LISTING).expect("第三挂单")),
        TestDepositReceipt::OperationsHold(
            OperationsHoldReceipt::new(
                CommandId::from_uuid(command(0x3a)),
                OperationsHoldDisposition::HoldReleased,
                Uuid::parse_str(THIRD_LISTING).expect("第三挂单"),
                ListingVersion::new(5).expect("版本"),
                ListingState::HeldOffMarket,
                Some(uuid(0x3a0)),
                Some(HeldOffMarketReason::Review),
            )
            .expect("止售解除回执"),
        ),
    );
    add(
        0x3b,
        worker(),
        TestDepositTarget::Season { season_id: "2026-11".into(), calendar_version: [0x3b; 32] },
        TestDepositReceipt::SeasonNotice(SeasonNoticeReceipt::queued(
            CommandId::from_uuid(worker_command(0x3b)),
            "2026-11".into(),
            uuid(0x3b0),
            UserId::from_uuid(seller_id()),
            uuid(0x3b1),
        )),
    );
    add(
        0x3c,
        worker(),
        TestDepositTarget::Season { season_id: "2026-11".into(), calendar_version: [0x3c; 32] },
        TestDepositReceipt::SeasonClose(SeasonCloseReceipt::closed(
            CommandId::from_uuid(worker_command(0x3c)),
            "2026-11".into(),
            SeasonCloseTarget::Listing(listing_id()),
            vec![listing_id()],
        )),
    );
    let closed_command = command(0x3d);
    cases.push((
        TestDepositLocks {
            command_id: closed_command,
            sellers: vec![seller_id()],
            equipment: vec![ListingEquipmentIdentity::new("main", "45").expect("装备身份")],
            mail_keys: vec![mail_key()],
        },
        admin(),
        TestDepositTarget::Listing(Uuid::parse_str(CLOSABLE_LISTING).expect("可关闭挂单")),
        TestDepositReceipt::OperationsHold(
            OperationsHoldReceipt::new(
                CommandId::from_uuid(closed_command),
                OperationsHoldDisposition::Closed,
                Uuid::parse_str(CLOSABLE_LISTING).expect("可关闭挂单"),
                ListingVersion::new(3).expect("版本"),
                ListingState::Closed,
                None,
                None,
            )
            .expect("未代管下架回执"),
        ),
    ));
    cases
}

/// 4A 的每一种 v2 回执都必须经真实序列化被 000134 接受，并在读回后等于原值。
#[tokio::test]
async fn postgres_deposit_v2_receipts_roundtrip_every_type() {
    let database = seed().await;
    let store = database.store().await.expect("连接运行账号");
    for (index, (locks, actor, target, receipt)) in receipt_cases().into_iter().enumerate() {
        let original = receipt.clone();
        let decoded = store
            .test_deposit_roundtrip_receipt(locks, actor, target, [index as u8 + 0x40; 32], receipt)
            .await
            .unwrap_or_else(|error| panic!("回执类型 {index} 往返失败：{error:?}"));
        assert_eq!(decoded, original, "回执类型 {index} 往返不一致");
    }
}
