//! 4C 整封接受与首次隔离回执、观察追加的 PG 行为。

use std::sync::Arc;

use auction_application::consignment::{
    AppendQuarantineObservationCommand, ConsignmentError, DecideEnvelopeCommand, EnvelopeStore,
    ObservationReceipt, RecheckQuarantineCommand,
};
use auction_application::core::RequestHash;
use auction_application::mail::QuarantineRecheckStore;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation, EnvelopeObservation,
    LockedEnvelopeFacts, MatchedDepositItem, QuarantineReason, ReservationBinding,
    ReservationState, SenderIdentity,
};
use auction_domain::consignment::{ListingVersion, OriginalMailKey};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_storage::{
    DepositRepository, DepositSettings, ReservationSecretAad, ReservationSecretCrypto,
    ReservationSecretCryptoError, SealedReservationSecret,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::support::{
    ACCEPTED, ENVELOPE, LISTING, OBSERVATION, RESERVATION, SESSION, USER_1,
    listing_awaiting_deposit, mail_session, observation_and_asset, reservation_issued, succeeds,
    users_and_key,
};
use crate::support::TestDatabase;

pub(super) struct NoCrypto;

impl ReservationSecretCrypto for NoCrypto {
    fn seal(
        &self,
        _token: &auction_domain::consignment::deposit::TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }

    fn open(
        &self,
        _sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<auction_domain::consignment::deposit::TokenText, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }
}

pub(super) async fn repository(database: &TestDatabase) -> DepositRepository {
    DepositRepository::new(
        database.store().await.expect("运行账号"),
        Arc::new(NoCrypto),
        DepositSettings::new(
            auction_domain::consignment::FeeBps::new(500).expect("费率"),
            auction_domain::consignment::ListingPrice::new(1).expect("下限"),
            auction_domain::consignment::ListingPrice::new(1_000_000_000).expect("上限"),
            24,
            100,
        ),
    )
}

fn future_listing() -> String {
    listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
}

fn future_reservation() -> String {
    reservation_issued().replace("2026-01-01T12:00:00Z", "2099-01-01T00:00:00Z")
}

fn share_claim() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL,
                 '2026-01-01T02:00:00Z', '{RESERVATION}');"
    )
}

const RETURN_OBSERVATION: &str = "84000000-0000-0000-0000-000000000002";
const QUARANTINE_OBSERVATION: &str = "84000000-0000-0000-0000-000000000003";

fn observation_variants() -> String {
    format!(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{RETURN_OBSERVATION}', '12345', 'main', '555', 'obs-return', '{SESSION}',
                 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('66', 32), 'hex'), decode(repeat('67', 32), 'hex'),
                 decode(repeat('7a', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                    \"protected_ref\":\"{ENVELOPE}\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",
                   \"asset_count\":1,
                   \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":false,
                               \"history_ok\":true}}]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{QUARANTINE_OBSERVATION}', '12345', 'main', '555', 'obs-quarantine',
                 '{SESSION}', 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('66', 32), 'hex'), decode(repeat('67', 32), 'hex'),
                 decode(repeat('7b', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                    \"protected_ref\":\"{ENVELOPE}\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":true,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",
                   \"asset_count\":1,
                   \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}}]}}'::jsonb,
                 NULL);"
    )
}

pub(super) fn readiness() -> String {
    format!(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{SESSION}', 1, decode(repeat('35', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), timestamptz '2100-01-01T00:00:00Z', 1)
         ON CONFLICT (world_id) DO UPDATE
         SET session_id = excluded.session_id, allowed_senders = excluded.allowed_senders,
             take_ready = excluded.take_ready, return_ready = excluded.return_ready,
             valid_until = excluded.valid_until, version =
                 public.consignment_mail_readiness.version + 1;
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;"
    )
}

pub(super) async fn seed_base(pool: &PgPool) {
    succeeds(
        pool,
        &(users_and_key()
            + &future_listing()
            + &mail_session()
            + &future_reservation()
            + &observation_and_asset()
            + &observation_variants()
            + &share_claim()
            + &readiness()),
    )
    .await;
}

fn key() -> OriginalMailKey {
    OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键")
}

pub(super) fn observation(duplicate: bool) -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
        session_binding_digest: [0x33; 32],
        manifest_digest: [0x68; 32],
        parser_version: "parser-v1".to_owned(),
        subject_is_consign: true,
        sender_identity: SenderIdentity::Verified,
        list_detail_consistent: true,
        asset_count: Some(1),
        duplicate_raw_eid: duplicate,
        canonical_alias_ambiguous: false,
        unknown_attachment: false,
        credits: Some(0),
        cod: Some(0),
        returnable: true,
        takeable: Some(true),
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
            seller_user_id: Some(UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家"))),
            tradeable: true,
            history_ok: true,
            protected_ref: Some(Uuid::parse_str(ENVELOPE).expect("凭证")),
        }],
    }
}

fn reservation() -> ReservationBinding {
    ReservationBinding {
        reservation_id: Uuid::parse_str(RESERVATION).expect("预约"),
        generation: 1,
        listing_id: Uuid::parse_str(LISTING).expect("挂单"),
        listing_version: ListingVersion::new(3).expect("版本"),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        equipment: auction_domain::consignment::ListingEquipmentIdentity::new("main", "42")
            .expect("装备"),
        expires_at: auction_time::Timestamp::parse_from_rfc3339("2099-01-01T00:00:00Z")
            .expect("截止"),
        state: ReservationState::Active,
    }
}

pub(super) fn accepted_command(command_id: CommandId) -> DecideEnvelopeCommand {
    let matched = MatchedDepositItem {
        ordinal: 1,
        listing_id: Uuid::parse_str(LISTING).expect("挂单"),
        listing_version: ListingVersion::new(3).expect("版本"),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        reservation_id: Uuid::parse_str(RESERVATION).expect("预约"),
        reservation_generation: 1,
        equipment: auction_domain::consignment::ListingEquipmentIdentity::new("main", "42")
            .expect("装备"),
        credential_ref: Some(Uuid::parse_str(ENVELOPE).expect("凭证")),
    };
    let facts = LockedEnvelopeFacts {
        mail_key: Some(key()),
        observation: observation(false),
        reservations: vec![reservation()],
        envelope_seller_user_id: Some(UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家"))),
        capabilities: auction_domain::consignment::deposit::EnvelopeCapabilities {
            safe_original_return: true,
            take_ready: true,
        },
        read_failure: false,
        already_decided: false,
        now: auction_time::Timestamp::UNIX_EPOCH,
    };
    DecideEnvelopeCommand {
        command_id,
        request_hash: RequestHash::new([7; 32]),
        mail_key: key(),
        facts,
        decision: EnvelopeDecision::Accept(vec![matched]),
    }
}

pub(super) fn return_command(command_id: CommandId) -> DecideEnvelopeCommand {
    let mut observation = observation(false);
    observation.observation_id = Uuid::parse_str(RETURN_OBSERVATION).expect("退回观察");
    observation.manifest_digest = [0x7a; 32];
    observation.items[0].tradeable = false;
    let facts = LockedEnvelopeFacts {
        mail_key: Some(key()),
        observation,
        reservations: Vec::new(),
        envelope_seller_user_id: None,
        capabilities: auction_domain::consignment::deposit::EnvelopeCapabilities {
            safe_original_return: true,
            take_ready: true,
        },
        read_failure: false,
        already_decided: false,
        now: auction_time::Timestamp::UNIX_EPOCH,
    };
    DecideEnvelopeCommand {
        command_id,
        request_hash: RequestHash::new([12; 32]),
        mail_key: key(),
        facts,
        decision: EnvelopeDecision::Return(auction_domain::consignment::deposit::ReturnManifest {
            items: vec![auction_domain::consignment::deposit::ReturnItem {
                ordinal: 1,
                canonical_eid: Some("42".to_owned()),
                reason: auction_domain::consignment::deposit::ReturnItemReason::NotTradeable,
            }],
        }),
    }
}

pub(super) fn quarantine_command(command_id: CommandId) -> DecideEnvelopeCommand {
    let mut observation = observation(true);
    observation.observation_id = Uuid::parse_str(QUARANTINE_OBSERVATION).expect("隔离观察");
    observation.manifest_digest = [0x7b; 32];
    let facts = LockedEnvelopeFacts {
        mail_key: Some(key()),
        observation,
        reservations: Vec::new(),
        envelope_seller_user_id: None,
        capabilities: auction_domain::consignment::deposit::EnvelopeCapabilities {
            safe_original_return: true,
            take_ready: true,
        },
        read_failure: false,
        already_decided: false,
        now: auction_time::Timestamp::UNIX_EPOCH,
    };
    DecideEnvelopeCommand {
        command_id,
        request_hash: RequestHash::new([8; 32]),
        mail_key: key(),
        facts,
        decision: EnvelopeDecision::Quarantine(QuarantineReason::DuplicateRawEid),
    }
}

pub(super) fn worker_command(text: &str) -> CommandId {
    CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, text.as_bytes()))
}

#[tokio::test]
async fn postgres_deposit_envelope_is_atomic() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let receipt = repository
        .decide_envelope(accepted_command(worker_command("decide-accept")))
        .await
        .expect("整封接受");
    assert_eq!(receipt.disposition, auction_application::consignment::ReceiptDisposition::Accepted);
    let existing = repository
        .test_read_existing_decision(&key())
        .await
        .expect("按 K 读取首次决定")
        .expect("首次决定存在");
    assert_eq!(existing, receipt);
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("deposit_accepted".to_owned(), 4));
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id=$1),
            (SELECT count(*) FROM public.consignment_custody_positions p
             JOIN public.consignment_deposit_items i ON i.id=p.item_id WHERE i.decision_id=$1),
            (SELECT count(*) FROM public.mail_disposition_work WHERE decision_id=$1 AND action='take')",
    )
    .bind(receipt.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取整封事实");
    assert_eq!(counts, (1, 1, 1));
    let conflict =
        repository.decide_envelope(accepted_command(worker_command("decide-other"))).await;
    assert!(conflict.is_err());
    let finals: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_final_dispositions")
        .fetch_one(pool)
        .await
        .expect("读取终局");
    assert_eq!(finals, 1);
}

#[tokio::test]
async fn postgres_decide_rejects_command_decision_mismatch() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let mut command = return_command(worker_command("decide-mismatch"));
    command.decision = EnvelopeDecision::ArchiveEmpty;
    let error = repository.decide_envelope(command).await.expect_err("命令决定不得替代锁内复算");
    assert_eq!(
        error,
        auction_application::consignment::ConsignmentError::Deposit(
            auction_application::consignment::DepositBusinessRejection::DispositionConflict
        )
    );
    let finals: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_final_dispositions")
        .fetch_one(pool)
        .await
        .expect("读取终局");
    assert_eq!(finals, 0);
    let works: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_disposition_work")
        .fetch_one(pool)
        .await
        .expect("读取工作");
    assert_eq!(works, 0);
}

#[tokio::test]
async fn postgres_decide_return_commits_one_return_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let command_id = worker_command("decide-return");
    let receipt = repository.decide_envelope(return_command(command_id)).await.expect("整封退回");
    assert_eq!(
        receipt.disposition,
        auction_application::consignment::ReceiptDisposition::ReturnDecided
    );
    let replay = repository.decide_envelope(return_command(command_id)).await.expect("回执重放");
    assert_eq!(replay, receipt);
    let works: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE action='return'),
                count(*) FILTER (WHERE action='take')
         FROM public.mail_disposition_work",
    )
    .fetch_one(pool)
    .await
    .expect("读取退回工作");
    assert_eq!(works, (1, 0));
}

#[tokio::test]
async fn postgres_disposition_delivery_ids_are_deterministic_v5() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let command_id = worker_command("decide-return-v5");
    let receipt = repository.decide_envelope(return_command(command_id)).await.expect("整封退回");
    let (delivery_id, delivery_command_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT d.id, d.command_id FROM public.deliveries AS d
         JOIN public.mail_disposition_work AS w ON w.delivery_id = d.id
         WHERE w.decision_id = $1 AND w.action = 'return'",
    )
    .bind(receipt.decision_id)
    .fetch_one(pool)
    .await
    .expect("读取投递身份");
    assert_eq!(delivery_id.get_version_num(), 5);
    assert_eq!(delivery_command_id.get_version_num(), 5);
    let business_key = format!("{}\0return\0", receipt.decision_id);
    let namespace = "auction.consignment.disposition.delivery.v1";
    let expected_delivery = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{namespace}\0delivery\0{business_key}").as_bytes(),
    );
    let expected_command = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{namespace}\0command\0{business_key}").as_bytes(),
    );
    assert_eq!(delivery_id, expected_delivery);
    assert_eq!(delivery_command_id, expected_command);
    let replay = repository.decide_envelope(return_command(command_id)).await.expect("同命令重放");
    assert_eq!(replay, receipt);
    let after: (Uuid, Uuid) = sqlx::query_as(
        "SELECT d.id, d.command_id FROM public.deliveries AS d
         JOIN public.mail_disposition_work AS w ON w.delivery_id = d.id
         WHERE w.decision_id = $1 AND w.action = 'return'",
    )
    .bind(receipt.decision_id)
    .fetch_one(pool)
    .await
    .expect("重放后读取投递身份");
    assert_eq!(after, (delivery_id, delivery_command_id));
}

#[tokio::test]
async fn postgres_decide_archive_empty_has_zero_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let observation_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', 'empty-observation', $2, 'fake-mailbox',
                 NULL, NULL, '2026-01-01T08:00:00Z', 'parser-empty', NULL,
                 decode(repeat('74', 32), 'hex'), decode(repeat('75', 32), 'hex'),
                 decode(repeat('76', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, false, 0, 0, NULL, '[]'::jsonb,
                 '{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T08:00\",\"asset_count\":0,
                   \"items\":[]}'::jsonb,
                 NULL)",
    )
    .bind(observation_id)
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .execute(pool)
    .await
    .expect("写入空信观察");
    let mut command = quarantine_command(worker_command("decide-archive-empty"));
    command.facts.observation = observation(false);
    command.facts.observation.observation_id = observation_id;
    command.facts.observation.manifest_digest = [0x76; 32];
    command.facts.observation.parser_version = "parser-empty".to_owned();
    command.facts.observation.asset_count = Some(0);
    command.facts.observation.items.clear();
    command.facts.observation.takeable = Some(false);
    command.decision = EnvelopeDecision::ArchiveEmpty;
    let repository = repository(&database).await;
    let receipt = repository.decide_envelope(command).await.expect("空信归档");
    assert_eq!(
        receipt.disposition,
        auction_application::consignment::ReceiptDisposition::ArchivedEmpty
    );
    let facts: (i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.mail_final_dispositions
             WHERE kind='archived_empty'),
            (SELECT count(*) FROM public.mail_disposition_work)",
    )
    .fetch_one(pool)
    .await
    .expect("读取空信事实");
    assert_eq!(facts, (1, 0));
}

#[tokio::test]
async fn postgres_quarantine_two_observations_first_receipt_replays() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first_command = worker_command("decide-quarantine");
    let first =
        repository.decide_envelope(quarantine_command(first_command)).await.expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    assert_eq!(quarantine.version, 1);
    let finals: i64 = sqlx::query_scalar("SELECT count(*) FROM public.mail_final_dispositions")
        .fetch_one(pool)
        .await
        .expect("读取隔离终局数");
    assert_eq!(finals, 0);
    let linked_initial: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取隔离内观察");
    let replay =
        repository.decide_envelope(quarantine_command(first_command)).await.expect("首回执重放");
    assert_eq!(replay, first);
    // 首次隔离已在同事务自动排入系统重检；这里直接领取，不重复排 Admin 工作。
    let claim = repository
        .claim_due(
            "quarantine-worker".to_owned(),
            auction_time::Timestamp::parse_from_rfc3339("2099-01-01T00:00:00Z").expect("锁后时刻"),
            auction_time::Timestamp::parse_from_rfc3339("2099-01-02T00:00:00Z").expect("租约"),
        )
        .await
        .expect("领取重检")
        .expect("应有一条工作");
    let second_observation = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', 'obs-2', $2, 'fake-mailbox', NULL, NULL,
                 '2026-01-01T04:00:00Z', 'parser-v2', NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'),
                 decode(repeat('69', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear\",
                    \"raw_eid\":\"42\",\"canonical_eid\":\"42\",
                    \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}]'::jsonb,
                 '{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":true,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T04:00\",
                   \"items\":[{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}]}'::jsonb,
                 NULL)",
    )
    .bind(second_observation)
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .execute(pool)
    .await
    .expect("写入第二条观察");
    let mut second = observation(true);
    second.observation_id = second_observation;
    second.manifest_digest = [0x69; 32];
    second.parser_version = "parser-v2".to_owned();
    let append_command = worker_command("append-observation");
    let appended = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: append_command,
            request_hash: RequestHash::new([10; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-2".to_owned(),
            observed_at: auction_time::Timestamp::parse_from_rfc3339("2026-01-01T04:00:00Z")
                .expect("观察时刻"),
            observation: second.clone(),
            evidence_ref: None,
        })
        .await
        .expect("追加观察");
    assert_eq!(appended.quarantine_version, 2);
    assert_ne!(appended.observation_id, Uuid::nil());
    let replay = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: append_command,
            request_hash: RequestHash::new([10; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-2".to_owned(),
            observed_at: auction_time::Timestamp::parse_from_rfc3339("2026-01-01T04:00:00Z")
                .expect("观察时刻"),
            observation: second.clone(),
            evidence_ref: None,
        })
        .await
        .expect("同命令重放");
    assert_eq!(replay, appended);
    let same_observation = repository
        .append_quarantine_observation(AppendQuarantineObservationCommand {
            command_id: worker_command("append-observation-again"),
            request_hash: RequestHash::new([11; 32]),
            quarantine_id: quarantine.quarantine_id,
            remote_observation_id: "obs-2".to_owned(),
            observed_at: auction_time::Timestamp::parse_from_rfc3339("2026-01-01T04:00:00Z")
                .expect("观察时刻"),
            observation: second,
            evidence_ref: None,
        })
        .await
        .expect("同观察重放");
    assert_eq!(same_observation.quarantine_version, 2);
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM public.mail_quarantines WHERE id=$1")
            .bind(quarantine.quarantine_id)
            .fetch_one(pool)
            .await
            .expect("读取隔离版本");
    assert_eq!(version, 2);
    assert_ne!(linked_initial, appended.observation_id);
    let _ = claim;
}

#[tokio::test]
async fn postgres_quarantine_admin_recheck_is_idempotent() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_base(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .decide_envelope(quarantine_command(worker_command("admin-quarantine-recheck")))
        .await
        .expect("首次隔离");
    let quarantine = first.quarantine.expect("隔离引用");
    let claim = repository
        .claim_due(
            "quarantine-worker".to_owned(),
            auction_time::Timestamp::parse_from_rfc3339("2099-01-01T00:00:00Z").expect("锁后时刻"),
            auction_time::Timestamp::parse_from_rfc3339("2099-01-02T00:00:00Z").expect("租约"),
        )
        .await
        .expect("领取自动重检")
        .expect("应有自动重检");
    let linked_initial: Uuid = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations WHERE quarantine_id=$1",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("隔离内观察");
    repository
        .complete(
            &claim,
            &ObservationReceipt::appended(
                CommandId::from_uuid(Uuid::new_v4()),
                quarantine.quarantine_id,
                1,
                linked_initial,
                [0x7c; 32],
            ),
        )
        .await
        .expect("收口自动重检");

    let command_id = CommandId::from_uuid(Uuid::new_v4());
    let command = RecheckQuarantineCommand {
        command_id,
        admin_actor: "fake-admin".to_owned(),
        quarantine_id: quarantine.quarantine_id,
        expected_version: 1,
        request_hash: RequestHash::new([9; 32]),
        request_id: None,
    };
    let queued = repository.recheck_quarantine(command.clone()).await.expect("Admin 重检排队");
    assert_eq!(queued.command_id, command_id);
    assert_eq!(repository.recheck_quarantine(command.clone()).await.expect("同 ID 重放"), queued);
    assert_eq!(
        repository
            .recheck_quarantine(RecheckQuarantineCommand {
                request_hash: RequestHash::new([0x0a; 32]),
                ..command
            })
            .await,
        Err(ConsignmentError::CommandIdConflict)
    );
    let queued_works: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.mail_quarantine_rechecks
         WHERE quarantine_id=$1 AND state='queued'",
    )
    .bind(quarantine.quarantine_id)
    .fetch_one(pool)
    .await
    .expect("读取排队重检");
    assert_eq!(queued_works, 1);
}
