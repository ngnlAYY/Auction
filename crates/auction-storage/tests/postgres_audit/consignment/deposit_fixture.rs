//! 审计失败事务的整封接受夹具：与 postgres 目标的收件夹具同形，但只在本独立进程使用。

use auction_application::consignment::{
    DecideEnvelopeCommand, ReservationSecretAad, ReservationSecretCrypto,
    ReservationSecretCryptoError, SealedReservationSecret,
};
use auction_application::core::RequestHash;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, EnvelopeItemKind, EnvelopeItemObservation,
    EnvelopeObservation, LockedEnvelopeFacts, MatchedDepositItem, ReservationBinding,
    ReservationState, SenderIdentity,
};
use auction_domain::consignment::{ListingVersion, OriginalMailKey};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_storage::{DepositRepository, DepositSettings};
use auction_time::{SecondsFormat, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

pub(super) const USER_1: &str = "10000000-0000-0000-0000-000000000001";
pub(super) const LISTING: &str = "20000000-0000-0000-0000-000000000001";
pub(super) const SNAPSHOT: &str = "30000000-0000-0000-0000-000000000001";
pub(super) const REPLAY: &str = "90000000-0000-0000-0000-000000000001";
pub(super) const REGISTER: &str = "40000000-0000-4000-8000-000000000001";
pub(super) const PUBLISH: &str = "41000000-0000-5000-8000-000000000001";
pub(super) const ISSUE: &str = "42000000-0000-4000-8000-000000000001";
pub(super) const CREDENTIAL: &str = "80000000-0000-0000-0000-000000000001";
pub(super) const SESSION: &str = "81000000-0000-0000-0000-000000000001";
pub(super) const RESERVATION: &str = "82000000-0000-0000-0000-000000000001";
pub(super) const ENVELOPE: &str = "83000000-0000-0000-0000-000000000001";
pub(super) const OBSERVATION: &str = "84000000-0000-0000-0000-000000000001";
pub(super) const ASSET: &str = "85000000-0000-0000-0000-000000000001";
pub(super) const START: &str = "2026-01-01T00:00:00Z";
pub(super) const LATER: &str = "2026-01-01T01:00:00Z";
pub(super) const ISSUED: &str = "2026-01-01T02:00:00Z";
pub(super) const ACCEPTED: &str = "2026-01-01T03:00:00Z";

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

pub(super) fn repository(store: auction_storage::DatabaseStore) -> DepositRepository {
    DepositRepository::new(
        store,
        std::sync::Arc::new(NoCrypto),
        DepositSettings::new(
            auction_domain::consignment::FeeBps::new(500).expect("费率"),
            auction_domain::consignment::ListingPrice::new(1).expect("下限"),
            auction_domain::consignment::ListingPrice::new(1_000_000_000).expect("上限"),
            24,
            100,
        ),
    )
}

fn key() -> OriginalMailKey {
    OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键")
}

pub(super) fn accepted_command(
    command_id: CommandId,
    reservation_expires_at: Timestamp,
) -> DecideEnvelopeCommand {
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
    DecideEnvelopeCommand {
        command_id,
        request_hash: RequestHash::new([7; 32]),
        mail_key: key(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(key()),
            observation: EnvelopeObservation {
                observation_id: Uuid::parse_str(OBSERVATION).expect("观察"),
                session_binding_digest: [0x33; 32],
                manifest_digest: [0x68; 32],
                parser_version: "parser-v1".to_owned(),
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
            },
            reservations: vec![ReservationBinding {
                reservation_id: Uuid::parse_str(RESERVATION).expect("预约"),
                generation: 1,
                listing_id: Uuid::parse_str(LISTING).expect("挂单"),
                listing_version: ListingVersion::new(3).expect("版本"),
                seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
                equipment: auction_domain::consignment::ListingEquipmentIdentity::new("main", "42")
                    .expect("装备"),
                expires_at: reservation_expires_at,
                state: ReservationState::Active,
            }],
            envelope_seller_user_id: Some(UserId::from_uuid(
                Uuid::parse_str(USER_1).expect("卖家"),
            )),
            capabilities: EnvelopeCapabilities { safe_original_return: true, take_ready: true },
            read_failure: false,
            already_decided: false,
            now: auction_time::Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::Accept(vec![matched]),
    }
}

fn seed_sql(future: &str) -> String {
    format!(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES
            ('{USER_1}', 'FakeSellerOne', decode(repeat('11', 32), 'hex'));
         INSERT INTO public.consignment_replay_keys
            (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('{REPLAY}', true, decode(repeat('13', 48), 'hex'),
                 decode(repeat('14', 12), 'hex'), 'fake-data-key', '{START}', '{START}');
         INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{LISTING}', '{USER_1}', 'main', '42', 'consigned', 'resolving', 101,
                 1, 500, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '{future}');
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{REGISTER}', 'public_user', '{USER_1}', 'register', 'new',
                 decode(repeat('15', 32), 'hex'), '{REPLAY}', '{LISTING}', 202, 'accepted',
                 '{{\"command_id\":\"{REGISTER}\",\"operation\":\"register\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":1,
                    \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                 '{START}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 1, '{REGISTER}', 'public_user', 'registered', NULL,
                 'resolving', NULL, 101, '{START}');
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{SNAPSHOT}', '{LISTING}', 1, 'hvdata-equip-v1', '{LATER}',
                 decode(repeat('16', 32), 'hex'), 'main', '42', 'Fake Gear', 'Weapon',
                 NULL, 'assigned', 100, true, 'FakeSellerOne', '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{PUBLISH}', 'worker', 'fake-worker', 'apply_lookup', '{LISTING}',
                 decode(repeat('17', 32), 'hex'), NULL, '{LISTING}', 200, 'ok',
                 '{{\"command_id\":\"{PUBLISH}\",\"operation\":\"apply_lookup\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":2,
                    \"state\":\"awaiting_deposit\",\"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"{future}\",\"reason_code\":null}}', '{LATER}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 2, '{PUBLISH}', 'worker', 'published', 'resolving',
                 'awaiting_deposit', NULL, 101, '{LATER}');
         UPDATE public.consignment_listings SET state='awaiting_deposit', state_version=2,
                 current_snapshot_id='{SNAPSHOT}', updated_at='{LATER}'
         WHERE id='{LISTING}';
         INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ('{CREDENTIAL}', 'mail_cookie', 'main', 'fake', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('a', 64),
                 'active', 1, '{START}', '{START}');
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('{SESSION}', '12345', 'main', 'FakeBot', '{CREDENTIAL}', 1, 1,
                 '{LATER}', 'hv_uid_forum_profile_armory_v1',
                 decode(repeat('33', 32), 'hex'));
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{ISSUE}', 2, 'public_user', '{USER_1}', 'issue_reservation', '{LISTING}',
                 decode(repeat('44', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', 201, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"issue_reservation\",
                    \"disposition\":\"reservation_issued\",\"command_id\":\"{ISSUE}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":3,
                    \"state\":\"awaiting_deposit\",\"reservation_id\":\"{RESERVATION}\",
                    \"expires_at\":\"{future}\"}}', '{ISSUED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 3, '{ISSUE}', 'public_user', 'reservation_issued',
                 'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
         UPDATE public.consignment_listings SET state_version=3, updated_at='{ISSUED}'
         WHERE id='{LISTING}';
         INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ('{RESERVATION}', '{LISTING}', '{USER_1}', 'FakeSellerOne', '12345', 'main',
                 '42', NULL, 1, 1, '{SESSION}', decode(repeat('55', 32), 'hex'),
                 '{ISSUED}', '{future}', 'active', '{ISSUE}', NULL, NULL);
         INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, clock_timestamp(), '{RESERVATION}');
         INSERT INTO public.mail_evidence_envelopes
            (id, purpose, aad_owner, ciphertext, nonce, encryption_key_id, fingerprint,
             created_at, updated_at, retired_at)
         VALUES ('{ENVELOPE}', 'credential', 'consignment', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('b', 64),
                 '{START}', '{START}', NULL);
         INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12345', 'main', '555', '{SESSION}', '2026-01-01T03:00', 'UTC',
                 '{ACCEPTED}', NULL);
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{OBSERVATION}', '12345', 'main', '555', 'obs-1', '{SESSION}',
                 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('66', 32), 'hex'), decode(repeat('67', 32), 'hex'),
                 decode(repeat('68', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
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
                   \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}}]}}'::jsonb,
                 NULL);
         INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ('{ASSET}', 'main', '42', NULL, '{OBSERVATION}', NULL);
         INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{SESSION}', 1, decode(repeat('35', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), timestamptz '2100-01-01T00:00:00Z', 1)
         ON CONFLICT (world_id) DO NOTHING;
         UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1;"
    )
}

pub(super) async fn seed(pool: &PgPool) -> Timestamp {
    let expires_at: Timestamp = sqlx::query_scalar("SELECT clock_timestamp() + interval '30 days'")
        .fetch_one(pool)
        .await
        .expect("生成相对截止时刻");
    let future = expires_at.to_rfc3339_opts(SecondsFormat::Micros, true);
    sqlx::raw_sql(sqlx::AssertSqlSafe(seed_sql(&future)))
        .execute(pool)
        .await
        .expect("写入审计夹具");
    expires_at
}
