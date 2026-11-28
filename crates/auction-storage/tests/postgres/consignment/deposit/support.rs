//! 寄售收件图的最小合法夹具：固定 ID、单顶层事务、可替换的单条事实。

use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};

use crate::support::fixtures;

pub const USER_1: &str = "10000000-0000-0000-0000-000000000001";
pub const USER_2: &str = "10000000-0000-0000-0000-000000000002";
pub const LISTING: &str = "20000000-0000-0000-0000-000000000001";
pub const SNAPSHOT: &str = "30000000-0000-0000-0000-000000000001";
pub const REPLAY: &str = "90000000-0000-0000-0000-000000000001";
pub const REGISTER: &str = "40000000-0000-4000-8000-000000000001";
pub const PUBLISH: &str = "41000000-0000-5000-8000-000000000001";
pub const DECIDE: &str = "41000000-0000-5000-8000-000000000002";
pub const ISSUE: &str = "42000000-0000-4000-8000-000000000001";
pub const CREDENTIAL: &str = "80000000-0000-0000-0000-000000000001";
pub const SESSION: &str = "81000000-0000-0000-0000-000000000001";
pub const RESERVATION: &str = "82000000-0000-0000-0000-000000000001";
pub const ENVELOPE: &str = "83000000-0000-0000-0000-000000000001";
pub const OBSERVATION: &str = "84000000-0000-0000-0000-000000000001";
pub const ASSET: &str = "85000000-0000-0000-0000-000000000001";
pub const DECISION: &str = "87000000-0000-0000-0000-000000000001";
pub const ITEM: &str = "88000000-0000-0000-0000-000000000001";
pub const POSITION: &str = "89000000-0000-0000-0000-000000000001";
pub const DELIVERY: &str = "8a000000-0000-0000-0000-000000000001";
pub const DELIVERY_COMMAND: &str = "8b000000-0000-0000-0000-000000000001";
pub const WORK: &str = "8c000000-0000-0000-0000-000000000001";
pub const ATTEMPT: &str = "9a000000-0000-4000-8000-000000000001";
pub const PROOF: &str = "9b000000-0000-4000-8000-000000000001";
pub const PROOF_COMMAND: &str = "9c000000-0000-5000-8000-000000000001";
pub const RELEASE_COMMAND: &str = "9d000000-0000-4000-8000-000000000001";
pub const RELEASE_DECISION: &str = "9e000000-0000-4000-8000-000000000001";
pub const RELEASE: &str = "9f000000-0000-4000-8000-000000000001";
pub const EXPIRE_COMMAND: &str = "41000000-0000-5000-8000-000000000003";
pub const REISSUE_COMMAND: &str = "41000000-0000-4000-8000-000000000004";
pub const CONTROL_INTENT: &str = "a0000000-0000-4000-8000-000000000001";
pub const CONTROL_HOLD: &str = "a1000000-0000-4000-8000-000000000001";
pub const CONTROL_RELEASE: &str = "a2000000-0000-4000-8000-000000000001";
pub const HANDOFF_EDGE: &str = "a3000000-0000-4000-8000-000000000001";
pub const NOTICE: &str = "a4000000-0000-4000-8000-000000000001";
pub const QUARANTINE_OBSERVATION: &str = "8f000000-0000-0000-0000-000000000002";
pub const START: &str = "2026-01-01T00:00:00Z";
pub const LATER: &str = "2026-01-01T01:00:00Z";
pub const ISSUED: &str = "2026-01-01T02:00:00Z";
pub const ACCEPTED: &str = "2026-01-01T03:00:00Z";
pub const END: &str = "2026-01-02T00:00:00Z";

pub async fn execute(pool: &PgPool, sql: &str) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::raw_sql(AssertSqlSafe(sql.to_owned())).execute(&mut *transaction).await?;
    transaction.commit().await
}

pub async fn succeeds(pool: &PgPool, sql: &str) {
    if let Err(error) = execute(pool, sql).await {
        let db_error = error.as_database_error();
        panic!(
            "应提交成功，SQLSTATE={:?}，约束={:?}",
            db_error.and_then(sqlx::error::DatabaseError::code),
            db_error.and_then(sqlx::error::DatabaseError::constraint),
        );
    }
}

pub async fn fails(pool: &PgPool, sql: &str, state: &str, constraint: Option<&str>) {
    let error = match execute(pool, sql).await {
        Ok(()) => panic!("语句应失败但已提交：{sql}"),
        Err(error) => error,
    };
    let db_error = error.as_database_error().expect("期望服务端 SQLSTATE");
    assert_eq!(
        db_error.code().as_deref(),
        Some(state),
        "SQL：{sql}；约束 {:?}",
        db_error.constraint()
    );
    if let Some(constraint) = constraint {
        assert_eq!(db_error.constraint(), Some(constraint), "SQL：{sql}");
    }
}

pub async fn check(pool: &PgPool, sql: &str, constraint: &str) {
    fails(pool, sql, "23514", Some(constraint)).await;
}

pub fn users_and_key() -> String {
    format!(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES
            ('{USER_1}', 'FakeSellerOne', decode(repeat('11', 32), 'hex')),
            ('{USER_2}', 'FakeSellerTwo', decode(repeat('12', 32), 'hex'));
         INSERT INTO public.consignment_replay_keys
            (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('{REPLAY}', true, decode(repeat('13', 48), 'hex'),
                 decode(repeat('14', 12), 'hex'), 'fake-data-key', '{START}', '{START}');"
    )
}

pub fn listing_awaiting_deposit() -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{LISTING}', '{USER_1}', 'main', '42', 'consigned', 'resolving', 101,
                 1, 500, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '{END}');
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
                    \"expires_at\":\"{END}\",\"reason_code\":null}}', '{LATER}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 2, '{PUBLISH}', 'worker', 'published', 'resolving',
                 'awaiting_deposit', NULL, 101, '{LATER}');
         UPDATE public.consignment_listings SET state='awaiting_deposit', state_version=2,
                 current_snapshot_id='{SNAPSHOT}', updated_at='{LATER}'
         WHERE id='{LISTING}';"
    )
}

pub fn mail_session() -> String {
    format!(
        "INSERT INTO public.credential_keys
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
                 decode(repeat('33', 32), 'hex'));"
    )
}

pub fn reservation_issued() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{ISSUE}', 2, 'public_user', '{USER_1}', 'issue_reservation', '{LISTING}',
                 decode(repeat('44', 32), 'hex'), 'listing', '{LISTING}', '{LISTING}', 201, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"issue_reservation\",
                    \"disposition\":\"reservation_issued\",\"command_id\":\"{ISSUE}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":3,
                    \"state\":\"awaiting_deposit\",\"reservation_id\":\"{RESERVATION}\",
                    \"expires_at\":\"2026-01-01T12:00:00Z\"}}', '{ISSUED}');
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
                 '{ISSUED}', '2026-01-01T12:00:00Z', 'active', '{ISSUE}', NULL, NULL);"
    )
}

/// 本事务内退休预约的 `expire_reservation` 命令行。
pub fn expire_reservation_command() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{EXPIRE_COMMAND}', 2, 'worker', 'fake-worker', 'expire_reservation',
                 '{RESERVATION}', decode(repeat('5d', 32), 'hex'), 'reservation',
                 '{RESERVATION}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"expire_reservation\",
                    \"disposition\":\"reservation_retired\",\"command_id\":\"{EXPIRE_COMMAND}\",
                    \"reservation_id\":\"{RESERVATION}\"}}', '{ISSUED}');"
    )
}

/// 本事务内以换发命令撤销旧预约；新预约与旧预约都引用同一个命令行。
pub fn reissue_reservation_command(new_reservation: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{REISSUE_COMMAND}', 2, 'public_user', '{USER_1}', 'issue_reservation',
                 '{LISTING}', decode(repeat('5e', 32), 'hex'), 'listing', '{LISTING}',
                 '{LISTING}', 201, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"issue_reservation\",
                    \"disposition\":\"reservation_issued\",\"command_id\":\"{REISSUE_COMMAND}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":4,
                    \"state\":\"awaiting_deposit\",\"reservation_id\":\"{new_reservation}\",
                    \"expires_at\":\"2026-01-01T12:00:00Z\"}}', '{ISSUED}');"
    )
}

pub fn observation_and_asset() -> String {
    format!(
        "INSERT INTO public.mail_evidence_envelopes
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
                    \"protected_ref\":\"83000000-0000-0000-0000-000000000001\"}}]'::jsonb,
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
         VALUES ('{ASSET}', 'main', '42', NULL, '{OBSERVATION}', NULL);"
    )
}

pub fn decide_command() -> String {
    decide_command_as(DECIDE)
}

/// 同一 K 的第二条 decide 命令；用于验证接受事件必须属于最终处置命令。
pub fn decide_command_as(command_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_bot_uid, target_world_id, target_external_mid,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{command_id}', 2, 'worker', 'fake-worker', 'decide_envelope', '12345|main|555',
                 decode(repeat('70', 32), 'hex'), 'mail_key', '12345', 'main', '555', NULL,
                 200, 'ok', '{{\"schema_version\":2,\"operation\":\"decide_envelope\",
                    \"disposition\":\"accepted\",\"command_id\":\"{command_id}\",
                    \"decision_id\":\"{DECISION}\",
                    \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",
                    \"listing_version\":4,\"state\":\"deposit_accepted\"}}],
                    \"quarantine\":null}}', '{ACCEPTED}');"
    )
}

pub fn initial_and_final() -> String {
    format!(
        "INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12345', 'main', '555', '86000000-0000-0000-0000-000000000001',
                 '{OBSERVATION}', '{DECIDE}', 'accepted', NULL, '{ACCEPTED}');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12345', 'main', '555', '{DECISION}', 'accepted', '{OBSERVATION}',
                 '{DECIDE}', '{ACCEPTED}');"
    )
}

pub fn item_position_claim() -> String {
    format!(
        "INSERT INTO public.consignment_deposit_items
            (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
             reservation_id, listing_id, seller_user_id, equipment_id,
             accepted_listing_version, accepted_price, credential_ref, observation_id)
         VALUES ('{ITEM}', '12345', 'main', '555', '{DECISION}', 1, '42', '{RESERVATION}',
                 '{LISTING}', '{USER_1}', '{ASSET}', 4, 101, '{ENVELOPE}', '{OBSERVATION}');
         INSERT INTO public.consignment_custody_positions
            (id, item_id, listing_id, seller_user_id, equipment_id, world_id, canonical_eid,
             season_window_id, state, version, current_generation, updated_at)
         VALUES ('{POSITION}', '{ITEM}', '{LISTING}', '{USER_1}', '{ASSET}', 'main', '42',
                 NULL, 'awaiting_take', 1, NULL, '{ACCEPTED}');
         INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, NULL, '{POSITION}', '{ACCEPTED}', '{POSITION}');"
    )
}

pub fn consume_reservation() -> String {
    format!(
        "UPDATE public.consignment_deposit_reservations
         SET state='consumed', last_command_id='{DECIDE}'
         WHERE id='{RESERVATION}';"
    )
}

pub fn accept_listing() -> String {
    accept_listing_as(DECIDE)
}

/// 以指定命令写接受事件；调用方用于构造“事件来自另一条命令”的反例。
pub fn accept_listing_as(command_id: &str) -> String {
    accept_listing_as_with_actor(command_id, "worker")
}

/// 以指定命令与 actor 写接受事件；解除为 accept 时 actor 可为 admin/system。
pub fn accept_listing_as_with_actor(command_id: &str, actor_kind: &str) -> String {
    format!(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 4, '{command_id}', '{actor_kind}', 'deposit_accepted',
                 'awaiting_deposit', 'deposit_accepted', NULL, 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state='deposit_accepted', state_version=4, updated_at='{ACCEPTED}'
         WHERE id='{LISTING}';"
    )
}

pub fn delivery_and_work() -> String {
    format!(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ('{DELIVERY}', '{DELIVERY_COMMAND}', decode(repeat('69', 32), 'hex'),
                 'consignment_disposition', 'consignment:{DECISION}:{WORK}', 'FakeBot',
                 '{{\"schema_version\":2,\"operation\":\"take\",
                    \"world\":\"main\",\"external_mail_id\":\"555\",
                    \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"main\",
                    \"external_mid\":\"555\"}},
                    \"decision_id\":\"{DECISION}\",\"work_id\":\"{WORK}\"}}'::jsonb,
                 'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.mail_disposition_work
            (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
             delivery_id, expected_observation_id, manifest_digest, created_at)
         VALUES ('{WORK}', '12345', 'main', '555', '{DECISION}', NULL, 'take',
                 '{DELIVERY}', '{OBSERVATION}', decode(repeat('68', 32), 'hex'),
                 '{ACCEPTED}');"
    )
}

pub fn custody_event() -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 1, '{DECIDE}', 'accepted', NULL, 'awaiting_take', NULL,
                 '{LISTING}', 4, '{ACCEPTED}');"
    )
}

pub fn attempt_start_proof_and_custody() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{PROOF_COMMAND}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                 '{ATTEMPT}', decode(repeat('a5', 32), 'hex'), 'original_attempt',
                 '{ATTEMPT}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                    \"disposition\":\"take_proven\",\"command_id\":\"{PROOF_COMMAND}\",
                    \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\"}}',
                 '{ACCEPTED}');
         INSERT INTO public.mail_disposition_attempts
            (id, work_id, delivery_id, fence, credential_id, credential_version,
             runtime_generation, session_id, expected_digest, prepared_at)
         VALUES ('{ATTEMPT}', '{WORK}', '{DELIVERY}', 1, '{CREDENTIAL}', 1, 1,
                 '{SESSION}', decode(repeat('a6', 32), 'hex'), '{ACCEPTED}');
         INSERT INTO public.mail_disposition_write_starts
            (work_id, decision_id, delivery_id, attempt_id, fence, started_at, command_id)
         VALUES ('{WORK}', '{DECISION}', '{DELIVERY}', '{ATTEMPT}', 1, '{ACCEPTED}',
                 '{PROOF_COMMAND}');
         INSERT INTO public.mail_disposition_proofs
            (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
             observation_id, validator_version, digest, conclusion, proven_at)
         VALUES ('{PROOF}', '{WORK}', '{DECISION}', '{DELIVERY}', 1, '{ATTEMPT}',
                 'exact_take', 'original', '{OBSERVATION}', 'v1',
                 decode(repeat('a7', 32), 'hex'), 'take_proven', '{ACCEPTED}');
         INSERT INTO public.consignment_disposition_generations
            (position_id, generation, purpose, predecessor, proof_id, service_epoch,
             blacklist_baseline_ids, season_window_id, created_command_id, created_at)
         VALUES ('{POSITION}', 1, 'first_sale', NULL, '{PROOF}', 1,
                 ARRAY[]::uuid[], NULL, '{DECIDE}', '{ACCEPTED}');
         UPDATE public.consignment_custody_positions
         SET state='in_custody', version=2, current_generation=1, updated_at='{ACCEPTED}'
         WHERE id='{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 2, '{PROOF_COMMAND}', 'in_custody', 'awaiting_take',
                 'in_custody', '{PROOF}', '{LISTING}', 4, '{ACCEPTED}');"
    )
}

pub fn controls_notice_and_handoff() -> String {
    format!(
        "INSERT INTO public.consignment_custody_controls
            (id, position_id, kind, predecessor_id, command_id, actor_ref, version, occurred_at)
         VALUES ('{CONTROL_INTENT}', '{POSITION}', 'withdrawal_intent', NULL, '{DECIDE}',
                 '{USER_1}', 1, '{ACCEPTED}');
         INSERT INTO public.consignment_custody_controls
            (id, position_id, kind, predecessor_id, command_id, actor_ref, version, occurred_at)
         VALUES ('{CONTROL_HOLD}', '{POSITION}', 'hold_set', NULL, '{DECIDE}',
                 'fake-admin', 2, '{ACCEPTED}');
         INSERT INTO public.consignment_custody_controls
            (id, position_id, kind, predecessor_id, command_id, actor_ref, version, occurred_at)
         VALUES ('{CONTROL_RELEASE}', '{POSITION}', 'hold_released', '{CONTROL_HOLD}',
                 '{DECIDE}', 'fake-admin', 3, '{ACCEPTED}');
         INSERT INTO public.consignment_season_notices
            (id, season_window_id, position_id, seller_user_id, kind, command_id, created_at)
         VALUES ('{NOTICE}', '8d000000-0000-0000-0000-000000000001', '{POSITION}',
                 '{USER_1}', 'preclose_withdrawal', '{DECIDE}', '{ACCEPTED}');
         INSERT INTO public.consignment_equipment_handoffs
            (id, equipment_id, world_id, canonical_eid, season_id, predecessor_id,
             from_user_id, to_user_id, kind, auction_lot_id, position_id, proof_ref,
             occurred_at)
         VALUES ('{HANDOFF_EDGE}', '{ASSET}', 'main', '42', NULL, NULL, '{USER_1}',
                 '{USER_1}', 'custody_acquired', NULL, '{POSITION}', 'controlled-proof-ref',
                 '{ACCEPTED}');"
    )
}

pub fn quarantine_release_fixture() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{RELEASE_COMMAND}', 2, 'admin', 'fake-admin', 'release_quarantine',
                 '91000000-0000-0000-0000-000000000001', decode(repeat('a8', 32), 'hex'),
                 'quarantine', '91000000-0000-0000-0000-000000000001', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                    \"disposition\":\"quarantine_released\",\"command_id\":\"{RELEASE_COMMAND}\",
                    \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                    \"release_id\":\"{RELEASE}\",\"decision_id\":\"{RELEASE_DECISION}\",
                    \"decision_kind\":\"archive_empty\"}}', '{ACCEPTED}');
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{QUARANTINE_OBSERVATION}', '12346', 'main', '556', 'obs-q-2',
                 '81000000-0000-0000-0000-000000000002', 'fake-mailbox',
                 '91000000-0000-0000-0000-000000000001', 1, '{LATER}', 'parser-v1', NULL,
                 decode(repeat('a9', 32), 'hex'), decode(repeat('aa', 32), 'hex'),
                 decode(repeat('ab', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, NULL, '[]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"asset_count\":0,
                   \"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12346', 'main', '556', '{RELEASE_DECISION}', 'archived_empty',
                 '{QUARANTINE_OBSERVATION}', '{RELEASE_COMMAND}', '{ACCEPTED}');
         INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ('{RELEASE}', '12346', 'main', '556',
                 '91000000-0000-0000-0000-000000000001', 1, '{QUARANTINE_OBSERVATION}',
                 '{RELEASE_DECISION}', 'archived_empty', '{RELEASE_COMMAND}', NULL, NULL, NULL,
                 '{ACCEPTED}');
         UPDATE public.mail_quarantines
         SET state='released', version=2, updated_at='{ACCEPTED}'
         WHERE id='91000000-0000-0000-0000-000000000001';"
    )
}

pub async fn seed_settlement_recovery_handoff(pool: &PgPool) {
    let at = START.parse::<Timestamp>().expect("测试时刻");
    let deadline = at + auction_time::Duration::hours(6);
    let auction_id = "b1000000-0000-4000-8000-000000000001";
    let equipment_id = "b0000000-0000-4000-8000-000000000001";
    let lot_id = "b3000000-0000-4000-8000-000000000001";
    let origin_command = "b2000000-0000-4000-8000-000000000001";
    let payment = "b4000000-0000-4000-8000-000000000001";
    let seller_payout = "b5000000-0000-4000-8000-000000000001";
    let buyer_refund = "b6000000-0000-4000-8000-000000000001";
    let return_mail = "b7000000-0000-4000-8000-000000000001";
    let recovery = "b8000000-0000-4000-8000-000000000001";
    let handoff = "b9000000-0000-4000-8000-000000000001";
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1::uuid, 'main', '777', 'Fake recovery handoff', 'Weapon',
                 '{}'::jsonb, $2)",
    )
    .bind(equipment_id)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入恢复交接装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1::uuid, 'main', 9901, 'Fake recovery handoff', 'live', $2, $2, $2, $2, $3)",
    )
    .bind(auction_id)
    .bind(deadline)
    .bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入恢复交接场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1::uuid, 'fake_recovery_handoff', $2::uuid, $3, 'accepted')",
    )
    .bind(origin_command)
    .bind(auction_id)
    .bind([0x31_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入恢复交接命令");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1::uuid, $2::uuid, $3::uuid, 1, 'sold', 100, 100, $4, 'Weapon', 1)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("写入恢复交接 lot");
    for (id, kind, state, recipient, identity, amount, prerequisite) in [
        (payment, "winner_equipment_cod", "ready", Some(USER_2), None, 100_i64, None),
        (
            seller_payout,
            "seller_payout",
            "blocked",
            None,
            Some("FakeSellerOne"),
            100,
            Some(payment),
        ),
        (buyer_refund, "buyer_refund", "contingent", Some(USER_2), None, 100, Some(payment)),
    ] {
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                 amount, carrier_credit, equipment_id, prerequisite_obligation_id)
             VALUES ($1::uuid, $2::uuid, $3::uuid, 'main', $4::uuid, 1, $5, $6, $7::text::uuid,
                     $8, $9, 0, $10::uuid, $11::text::uuid)",
        )
        .bind(id)
        .bind(auction_id)
        .bind(lot_id)
        .bind(origin_command)
        .bind(kind)
        .bind(state)
        .bind(recipient)
        .bind(identity)
        .bind(amount)
        .bind(equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入恢复交接义务");
    }
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         VALUES ($1::uuid, 'fake-mailbox', '556', 'main', 'delivery', $2, $3, 'accepted',
                 'fake_recovery_handoff', '{}'::jsonb, $3, 'MoogleMail', 'fake recovery handoff')",
    )
    .bind(return_mail)
    .bind([0x32_u8; 32].as_slice())
    .bind(at)
    .execute(pool)
    .await
    .expect("写入恢复交接入站邮件");
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
             buyer_refund_obligation_id, policy_version, payment_deadline_at, stage,
             state_version, return_mail_id, return_external_mail_id, created_at, updated_at)
         VALUES ($1::uuid, $2::uuid, $3::uuid, 'main', $4::uuid, $5::uuid, $6::uuid, NULL,
                 $7::uuid, $8::uuid, 1, $9, 'reclaiming', 5, $10::uuid, '556', $11, $11)",
    )
    .bind(recovery)
    .bind(auction_id)
    .bind(lot_id)
    .bind(equipment_id)
    .bind(USER_2)
    .bind(payment)
    .bind(seller_payout)
    .bind(buyer_refund)
    .bind(at + auction_time::Duration::days(7))
    .bind(return_mail)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入恢复交接恢复记录");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_handoffs
            (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id, obligation_id,
             observation_id, accepted_at)
         VALUES ($1::uuid, '12346', 'main', '556', $2::uuid, $3::uuid, $4::uuid, $5::uuid, $6)",
    )
    .bind(handoff)
    .bind(return_mail)
    .bind(recovery)
    .bind(payment)
    .bind("8f000000-0000-0000-0000-000000000001")
    .bind(at)
    .execute(pool)
    .await
    .expect("写入恢复交接事实");
}

pub fn accepted_envelope() -> String {
    users_and_key()
        + &listing_awaiting_deposit()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &initial_and_final()
        + &item_position_claim()
        + &consume_reservation()
        + &accept_listing()
        + &delivery_and_work()
        + &custody_event()
}

pub async fn seed_accepted(pool: &PgPool) {
    succeeds(pool, &accepted_envelope()).await;
}
