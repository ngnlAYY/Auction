//! 000134 的库结构、命名约束、只追加事实、列级投影与 acquire_return 失败关闭。

use super::support::{
    ACCEPTED, DECIDE, DECISION, END, ISSUED, ITEM, LATER, LISTING, OBSERVATION, POSITION, PROOF,
    PROOF_COMMAND, RESERVATION, SESSION, START, USER_1, WORK, accept_listing_as,
    attempt_start_proof_and_custody, check, controls_notice_and_handoff, decide_command,
    decide_command_as, delivery_and_work, execute, fails, initial_and_final, item_position_claim,
    listing_awaiting_deposit, mail_session, observation_and_asset, quarantine_release_fixture,
    reservation_issued, seed_accepted, seed_settlement_recovery_handoff, succeeds, users_and_key,
};
use crate::support::TestDatabase;

const TABLES: [&str; 29] = [
    "consignment_season_windows",
    "mail_evidence_sessions",
    "consignment_mail_readiness",
    "mail_originals",
    "mail_original_links",
    "mail_evidence_envelopes",
    "mail_envelope_observations",
    "mail_observation_conflicts",
    "mail_initial_decisions",
    "mail_quarantines",
    "mail_quarantine_rechecks",
    "mail_final_dispositions",
    "mail_quarantine_releases",
    "settlement_recovery_return_handoffs",
    "consignment_deposit_reservations",
    "consignment_reservation_secrets",
    "consignment_equipment_claims",
    "consignment_custody_assets",
    "consignment_deposit_items",
    "consignment_custody_positions",
    "consignment_disposition_generations",
    "consignment_custody_events",
    "consignment_custody_controls",
    "consignment_equipment_handoffs",
    "consignment_season_notices",
    "mail_disposition_work",
    "mail_disposition_attempts",
    "mail_disposition_write_starts",
    "mail_disposition_proofs",
];

const CRITICAL_CONSTRAINTS: [&str; 37] = [
    "c4_season_world_season_uq",
    "c4_session_identity_uq",
    "c4_readiness_senders_ck",
    "c4_original_pk",
    "c4_link_mailbox_uq",
    "c4_envelope_shape_ck",
    "c4_observation_manifest_ck",
    "c4_observation_derived_ck",
    "c4_conflict_pk",
    "c4_initial_decision_kind_ck",
    "c4_quarantine_state_ck",
    "c4_quarantine_responsibility_ck",
    "c4_recheck_shape_ck",
    "c4_final_kind_ck",
    "c4_release_branch_ck",
    "c4_handoff_key_uq",
    "c4_res_generation_uq",
    "c4_res_secret_shape_ck",
    "c4_claim_holder_ck",
    "c4_asset_identity_uq",
    "c4_item_ordinal_uq",
    "c4_item_reservation_uq",
    "c4_position_item_uq",
    "c4_position_state_ck",
    "c4_generation_number_ck",
    "c4_custody_event_state_ck",
    "c4_control_kind_ck",
    "c4_handoff_edge_source_ck",
    "c7_handoff_edge_kinds_ck",
    "c4_notice_business_uq",
    "c4_work_shape_ck",
    "c4_attempt_identity_uq",
    "c4_start_identity_uq",
    "c4_command_schema_version_ck",
    "c4_command_typed_target_ck",
    "consignment_command_receipt",
    "c4_delivery_consignment_payload_ck",
];

const FACTS: [&str; 21] = [
    "consignment_season_windows",
    "mail_evidence_sessions",
    "mail_originals",
    "mail_original_links",
    "mail_envelope_observations",
    "mail_observation_conflicts",
    "mail_initial_decisions",
    "mail_final_dispositions",
    "mail_quarantine_releases",
    "settlement_recovery_return_handoffs",
    "consignment_custody_assets",
    "consignment_deposit_items",
    "consignment_disposition_generations",
    "consignment_custody_events",
    "consignment_custody_controls",
    "consignment_equipment_handoffs",
    "consignment_season_notices",
    "mail_disposition_work",
    "mail_disposition_attempts",
    "mail_disposition_write_starts",
    "mail_disposition_proofs",
];

const PROJECTIONS: [&str; 8] = [
    "consignment_mail_readiness",
    "mail_evidence_envelopes",
    "mail_quarantines",
    "mail_quarantine_rechecks",
    "consignment_deposit_reservations",
    "consignment_reservation_secrets",
    "consignment_equipment_claims",
    "consignment_custody_positions",
];

const FACT_UPDATE_COLUMNS: [(&str, &str); 21] = [
    ("consignment_season_windows", "season_id"),
    ("mail_evidence_sessions", "bot_uid"),
    ("mail_originals", "received_minute_raw"),
    ("mail_original_links", "mailbox_key"),
    ("mail_envelope_observations", "remote_observation_id"),
    ("mail_observation_conflicts", "evidence_ref"),
    ("mail_initial_decisions", "kind"),
    ("mail_final_dispositions", "kind"),
    ("mail_quarantine_releases", "expected_version"),
    ("settlement_recovery_return_handoffs", "accepted_at"),
    ("consignment_custody_assets", "canonical_eid"),
    ("consignment_deposit_items", "ordinal"),
    ("consignment_disposition_generations", "purpose"),
    ("consignment_custody_events", "event_kind"),
    ("consignment_custody_controls", "actor_ref"),
    ("consignment_equipment_handoffs", "proof_ref"),
    ("consignment_season_notices", "kind"),
    ("mail_disposition_work", "action"),
    ("mail_disposition_attempts", "expected_digest"),
    ("mail_disposition_write_starts", "fence"),
    ("mail_disposition_proofs", "digest"),
];

pub(crate) fn additional_rows() -> String {
    format!(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ('8d000000-0000-0000-0000-000000000001', '2026-1', 'isekai',
                 'America/Los_Angeles', '{START}', '{END}',
                 '2026-01-01T12:00:00Z', decode(repeat('71', 32), 'hex'), 3);
         INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', '{SESSION}', 1, decode(repeat('72', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne'], ARRAY['v1'], true, true, true,
                 '{LATER}', '{END}', 1);
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('81000000-0000-0000-0000-000000000002', '12346', 'main', 'FakeBotTwo',
                 '80000000-0000-0000-0000-000000000001', 1, 1, '{LATER}', 'v1',
                 decode(repeat('7d', 32), 'hex'));
         INSERT INTO public.consignment_reservation_secrets
            (reservation_id, generation, seller_user_id, ciphertext, nonce, encryption_key_id,
             fingerprint, created_at, updated_at, retired_at)
         VALUES ('{RESERVATION}', 1, '{USER_1}', decode('11', 'hex'),
                 decode(repeat('22', 12), 'hex'), 'fake-data-key', repeat('c', 64),
                 '{ISSUED}', '{ISSUED}', NULL);
         INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ('8e000000-0000-0000-0000-000000000001', 'fake-mailbox', '555', 'main',
                 'delivery', decode(repeat('73', 32), 'hex'), '{ACCEPTED}', 'accepted',
                 'fake_code', '{{}}'::jsonb, '{ACCEPTED}', 'FakeSellerOne', 'consign');
         INSERT INTO public.mail_original_links
            (inbound_id, mailbox_key, external_mid, bot_uid, world_id, auth_observation_id)
         VALUES ('8e000000-0000-0000-0000-000000000001', 'fake-mailbox', '555', '12345',
                 'main', '{OBSERVATION}');
         INSERT INTO public.mail_observation_conflicts
            (observation_id, conflicting_digest, evidence_ref, observed_at)
         VALUES ('{OBSERVATION}', decode(repeat('74', 32), 'hex'), 'controlled-ref',
                 '{ACCEPTED}');
         INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12346', 'main', '556', '81000000-0000-0000-0000-000000000002',
                 '2026-01-01T03:00', 'UTC',
                 '{ACCEPTED}', NULL);
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('8f000000-0000-0000-0000-000000000001', '12346', 'main', '556', 'obs-q-1',
                 '81000000-0000-0000-0000-000000000002', 'fake-mailbox', NULL, NULL,
                 '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('75', 32), 'hex'), decode(repeat('76', 32), 'hex'),
                 decode(repeat('77', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"unknown\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":true,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",
                   \"items\":[{{\"ordinal\":1,\"world\":\"main\",\"tradeable\":false,
                               \"history_ok\":false}}]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12346', 'main', '556', '90000000-0000-0000-0000-000000000001',
                 '8f000000-0000-0000-0000-000000000001', '{DECIDE}', 'quarantined',
                 '91000000-0000-0000-0000-000000000001', '{ACCEPTED}');
         INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ('91000000-0000-0000-0000-000000000001', '12346', 'main', '556',
                 '90000000-0000-0000-0000-000000000001', 1, 'active',
                 'controlled-reread', 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('92000000-0000-4000-8000-000000000001', 2, 'admin', 'fake-admin',
                 'recheck_quarantine', '91000000-0000-0000-0000-000000000001',
                 decode(repeat('78', 32), 'hex'), 'quarantine',
                 '91000000-0000-0000-0000-000000000001', NULL, 202, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"recheck_quarantine\",
                    \"disposition\":\"recheck_queued\",\"command_id\":
                    \"92000000-0000-4000-8000-000000000001\",
                    \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                    \"work_id\":\"93000000-0000-0000-0000-000000000001\"}}',
                 '{ACCEPTED}');
         INSERT INTO public.mail_quarantine_rechecks
            (id, quarantine_id, requested_version, command_id, state, available_at,
             lease_token, lease_until, fence, attempt_count, completed_at)
         VALUES ('93000000-0000-0000-0000-000000000001',
                 '91000000-0000-0000-0000-000000000001', 1,
                 '92000000-0000-4000-8000-000000000001', 'queued', '{ACCEPTED}',
                 NULL, NULL, 0, 0, NULL);"
    )
}

#[tokio::test]
async fn postgres_consignment_deposit_schema_has_all_tables_and_critical_constraints() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class
         WHERE relnamespace = 'public'::regnamespace AND relkind = 'r'
           AND relname = ANY($1) ORDER BY relname",
    )
    .bind(TABLES.as_slice())
    .fetch_all(pool)
    .await
    .expect("读取新表");
    assert_eq!(tables.len(), TABLES.len(), "缺少新表：{:?}", TABLES);
    let constraints: Vec<String> = sqlx::query_scalar(
        "SELECT conname::text FROM pg_constraint
         WHERE connamespace = 'public'::regnamespace AND conname = ANY($1)
         ORDER BY conname",
    )
    .bind(CRITICAL_CONSTRAINTS.as_slice())
    .fetch_all(pool)
    .await
    .expect("读取命名约束");
    assert_eq!(
        constraints.len(),
        CRITICAL_CONSTRAINTS.len(),
        "缺少关键约束：{:?}",
        CRITICAL_CONSTRAINTS
    );
    let proof_indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes
         WHERE schemaname = 'public'
           AND indexname IN ('c4_proof_conclusion_uq', 'c4_proof_asset_uq')
         ORDER BY indexname",
    )
    .fetch_all(pool)
    .await
    .expect("读取证明索引");
    assert_eq!(proof_indexes, vec!["c4_proof_conclusion_uq".to_owned()]);
}

#[tokio::test]
async fn postgres_consignment_deposit_constraints_and_append_only_triggers_are_explicit() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let unnamed: Vec<(String, String)> = sqlx::query_as(
        "SELECT class.relname::text, constraint_row.conname::text
         FROM pg_constraint AS constraint_row
         JOIN pg_class AS class ON class.oid = constraint_row.conrelid
         WHERE class.relnamespace = 'public'::regnamespace
           AND class.relname = ANY($1)
           AND constraint_row.contype IN ('p', 'u', 'f', 'c')
           AND constraint_row.conname !~ '^c[4567]_'
         ORDER BY class.relname, constraint_row.conname",
    )
    .bind(TABLES.as_slice())
    .fetch_all(pool)
    .await
    .expect("读取未显式命名的约束");
    assert!(unnamed.is_empty(), "新表存在未显式命名的约束：{unnamed:?}");

    let missing: Vec<String> = sqlx::query_scalar(
        "SELECT expected.table_name || ':' || suffix.trigger_suffix
         FROM unnest($1::text[]) AS expected(table_name)
         CROSS JOIN (VALUES ('_append_only'), ('_no_truncate')) AS suffix(trigger_suffix)
         WHERE NOT EXISTS (
             SELECT 1
             FROM pg_trigger AS trigger_row
             JOIN pg_class AS class ON class.oid = trigger_row.tgrelid
             WHERE class.relnamespace = 'public'::regnamespace
               AND class.relname = expected.table_name
               AND trigger_row.tgname = expected.table_name || suffix.trigger_suffix
         )
         ORDER BY 1",
    )
    .bind(FACTS.as_slice())
    .fetch_all(pool)
    .await
    .expect("读取缺失的只追加触发器");
    assert!(missing.is_empty(), "事实表缺少只追加触发器：{missing:?}");

    let projection_mutation: Vec<String> = sqlx::query_scalar(
        "SELECT class.relname::text || ':' || trigger_row.tgname::text
         FROM pg_trigger AS trigger_row
         JOIN pg_class AS class ON class.oid = trigger_row.tgrelid
         WHERE class.relnamespace = 'public'::regnamespace
           AND class.relname = ANY($1)
           AND (trigger_row.tgname LIKE '%_append_only' OR trigger_row.tgname LIKE '%_no_truncate')
         ORDER BY 1",
    )
    .bind(PROJECTIONS.as_slice())
    .fetch_all(pool)
    .await
    .expect("读取投影表上的只追加触发器");
    assert!(projection_mutation.is_empty(), "投影表不应有只追加触发器：{projection_mutation:?}");
}

#[tokio::test]
async fn postgres_consignment_deposit_schema_accepts_the_valid_graph_and_extra_projections() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;

    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_deposit_items),
            (SELECT count(*) FROM public.consignment_custody_positions),
            (SELECT count(*) FROM public.mail_quarantine_rechecks),
            (SELECT count(*) FROM public.mail_original_links),
            (SELECT count(*) FROM public.consignment_mail_readiness),
            (SELECT count(*) FROM public.consignment_reservation_secrets)",
    )
    .fetch_one(pool)
    .await
    .expect("读取合法图计数");
    assert_eq!(counts, (1, 1, 1, 1, 1, 1));
}

#[tokio::test]
async fn postgres_consignment_deposit_schema_accepts_extended_fact_graph() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    succeeds(pool, &controls_notice_and_handoff()).await;
    succeeds(pool, &quarantine_release_fixture()).await;
    seed_settlement_recovery_handoff(pool).await;

    let counts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_disposition_generations),
            (SELECT count(*) FROM public.consignment_custody_events),
            (SELECT count(*) FROM public.consignment_custody_controls),
            (SELECT count(*) FROM public.consignment_equipment_handoffs),
            (SELECT count(*) FROM public.mail_disposition_proofs),
            (SELECT count(*) FROM public.mail_quarantine_releases),
            (SELECT count(*) FROM public.settlement_recovery_return_handoffs)",
    )
    .fetch_one(pool)
    .await
    .expect("读取扩展事实图计数");
    assert_eq!(counts, (1, 2, 3, 1, 1, 1, 1));
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_disposition_generations
                (position_id, generation, purpose, predecessor, proof_id, service_epoch,
                 blacklist_baseline_ids, season_window_id, created_command_id, created_at)
             VALUES ('{POSITION}', 2, 'withdrawal_only', 1, '{PROOF}', 1,
                     ARRAY[]::uuid[], NULL, '{DECIDE}', '{ACCEPTED}');"
        ),
        // g+1 在 000137 放开：没有同事务的成功命令与 Withdrawal 合同时仍被延迟触发器拒绝。
        "c7_generation_successor_command",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ('{POSITION}', 3, '{PROOF_COMMAND}', 'lost', 'awaiting_take',
                     'lost', '{PROOF}', '{LISTING}', 4, '{ACCEPTED}');"
        ),
        // lost 事件在 000137 放开，来源态限于仍占用实物的四态。
        "c4_custody_event_state_ck",
    )
    .await;
}

#[tokio::test]
async fn postgres_accept_event_from_another_command_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let other_command = "9c000000-0000-5000-8000-0000000000aa";
    let sql = users_and_key()
        + &listing_awaiting_deposit()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &decide_command_as(other_command)
        + &initial_and_final()
        + &item_position_claim()
        + &super::support::consume_reservation()
        + &accept_listing_as(other_command)
        + &delivery_and_work()
        + &super::support::custody_event();
    fails(pool, &sql, "23514", Some("c4_envelope_accept_bindings")).await;
}

#[tokio::test]
async fn postgres_conclusive_proof_slot_is_partial_unique() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_proofs
                (id, work_id, decision_id, delivery_id, fence, attempt_id, kind, asset_leg,
                 observation_id, validator_version, digest, conclusion, proven_at)
             SELECT '9b000000-0000-4000-8000-000000000098', work_id, NULL, delivery_id, fence,
                    attempt_id, 'exact_return', 'original', observation_id, validator_version,
                    digest, 'return_proven', proven_at
             FROM public.mail_disposition_proofs WHERE id='{PROOF}';"
        ),
        "23505",
        Some("c4_proof_conclusion_uq"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_reservation_deadline_rejects_beyond_the_listing() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &super::support::users_and_key()).await;
    succeeds(pool, &super::support::listing_awaiting_deposit()).await;
    succeeds(pool, &super::support::mail_session()).await;
    succeeds(pool, &super::support::reservation_issued()).await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             VALUES ('94000000-0000-0000-0000-000000000001', '{LISTING}', '{USER_1}',
                     'FakeSellerOne', '12345', 'main', '42', NULL, 2, 1, '{SESSION}',
                     decode(repeat('80', 32), 'hex'), '{ISSUED}',
                     '2026-01-03T00:00:00Z', 'active',
                     '42000000-0000-4000-8000-000000000001', NULL, NULL);"
        ),
        "c4_res_deadline",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_command_v2_shape_keeps_the_v1_branch() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;

    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, replay_key_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('b0000000-0000-4000-8000-000000000001', 1, 'public_user', '{USER_1}',
                     'register', 'new', decode(repeat('c1', 32), 'hex'), 'listing',
                     '90000000-0000-0000-0000-000000000001', '{LISTING}', 202, 'accepted',
                     '{{\"command_id\":\"b0000000-0000-4000-8000-000000000001\",
                        \"operation\":\"register\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":1,\"state\":\"resolving\",
                        \"disposition\":\"registered\",\"price\":101}}', '{ACCEPTED}');"
        ),
        "c4_command_typed_target_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, target_bot_uid, listing_id,
                 result_status, result_code, result_body, created_at)
             VALUES ('b0000000-0000-4000-8000-000000000002', 2, 'public_user', '{USER_1}',
                     'issue_reservation', '{LISTING}', decode(repeat('c2', 32), 'hex'),
                     'listing', '{LISTING}', 'unexpected', '{LISTING}', 201, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"issue_reservation\",
                        \"disposition\":\"reservation_issued\",
                        \"command_id\":\"b0000000-0000-4000-8000-000000000002\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":3,
                        \"state\":\"awaiting_deposit\",
                        \"reservation_id\":\"82000000-0000-0000-0000-000000000002\",
                        \"expires_at\":\"2026-01-01T12:00:00Z\"}}', '{ACCEPTED}');"
        ),
        "c4_command_typed_target_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('b0000000-0000-4000-8000-000000000003', 2, 'admin', 'fake-admin',
                     'withdraw', '{LISTING}', decode(repeat('c3', 32), 'hex'), 'listing',
                     '{LISTING}', '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"b0000000-0000-4000-8000-000000000003\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":4,
                        \"state\":\"deposit_accepted\",
                        \"intent_id\":\"b1000000-0000-4000-8000-000000000003\"}}',
                     '{ACCEPTED}');"
        ),
        "consignment_command_actor",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('b0000000-0000-4000-8000-000000000004', 2, 'public_user', '{USER_1}',
                     'issue_reservation', '{LISTING}', decode(repeat('c4', 32), 'hex'),
                     'listing', '{LISTING}', '{LISTING}', 201, 'bad',
                     '{{\"schema_version\":2,\"operation\":\"issue_reservation\",
                        \"disposition\":\"reservation_issued\",
                        \"command_id\":\"b0000000-0000-4000-8000-000000000004\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":3,
                        \"state\":\"awaiting_deposit\",
                        \"reservation_id\":\"82000000-0000-0000-0000-000000000002\",
                        \"expires_at\":\"2026-01-01T12:00:00Z\"}}', '{ACCEPTED}');"
        ),
        "consignment_command_result_code",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('b0000000-0000-4000-8000-000000000005', 2, 'admin', 'fake-admin',
                     'release_hold', '{LISTING}', decode(repeat('c5', 32), 'hex'), 'listing',
                     '{LISTING}', '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_hold\",
                        \"disposition\":\"hold_released\",
                        \"command_id\":\"b0000000-0000-4000-8000-000000000005\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                        \"state\":\"available\",
                        \"hold_id\":\"b1000000-0000-4000-8000-000000000005\"}}',
                     '{ACCEPTED}');"
        ),
        "consignment_command_receipt",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_v2_withdraw_receipt_accepts_both_results() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let command_id = "c4000000-0000-4000-8000-000000000001";
    let listing_id = "c4000000-0000-4000-8000-000000000002";
    let intent_id = "c4000000-0000-4000-8000-000000000003";

    let accepts = |status: i32, state: &str, disposition: &str, with_intent: bool| {
        let mut body = serde_json::json!({
            "schema_version": 2,
            "operation": "withdraw",
            "disposition": disposition,
            "command_id": command_id,
            "listing_id": listing_id,
            "listing_version": 4,
            "state": state,
        });
        if with_intent {
            body["intent_id"] = serde_json::json!(intent_id);
        }
        (status, body)
    };
    for (status, state, disposition, with_intent) in [
        (202, "deposit_accepted", "withdrawal_requested", true),
        (202, "withdrawal_pending", "withdrawal_requested", true),
        (202, "held_off_market", "withdrawal_requested", true),
        (200, "closed", "closed", false),
    ] {
        let (status, body) = accepts(status, state, disposition, with_intent);
        let valid: bool = sqlx::query_scalar(
            "SELECT public.c4_valid_v2_receipt($1::jsonb, $2::uuid, 'withdraw', $3)",
        )
        .bind(body)
        .bind(command_id)
        .bind(status)
        .fetch_one(pool)
        .await
        .expect("调用 v2 回执校验");
        assert!(valid, "withdraw/{state}/{disposition} 应被接受");
    }
    for (status, state, disposition, with_intent) in [
        (202, "available", "withdrawal_requested", true),
        (200, "available", "closed", false),
        (200, "closed", "closed", true),
    ] {
        let (status, body) = accepts(status, state, disposition, with_intent);
        let valid: bool = sqlx::query_scalar(
            "SELECT public.c4_valid_v2_receipt($1::jsonb, $2::uuid, 'withdraw', $3)",
        )
        .bind(body)
        .bind(command_id)
        .bind(status)
        .fetch_one(pool)
        .await
        .expect("调用 v2 回执校验");
        assert!(!valid, "withdraw/{state}/{disposition} 应被拒绝");
    }
}

#[tokio::test]
async fn postgres_consignment_deposit_delivery_payload_requires_canonical_decision_and_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let decision_id = "c5000000-0000-4000-8000-000000000001";
    let work_id = "c5000000-0000-4000-8000-000000000002";
    let insert = |delivery_id: &str, decision: &str, work: &str| {
        format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('{delivery_id}', '{delivery_id}', decode(repeat('c5', 32), 'hex'),
                     'consignment_disposition', 'consignment:{decision}:{work}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"take\",
                        \"world\":\"main\",\"external_mail_id\":\"555\",
                        \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"main\",
                        \"external_mid\":\"555\"}},
                        \"decision_id\":\"{decision}\",\"work_id\":\"{work}\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');"
        )
    };
    succeeds(pool, &insert("c5000000-0000-4000-8000-000000000010", decision_id, work_id)).await;
    fails(
        pool,
        &insert("c5000000-0000-4000-8000-000000000011", "not-a-uuid", work_id),
        "23514",
        Some("c4_delivery_consignment_payload_ck"),
    )
    .await;
    fails(
        pool,
        &insert(
            "c5000000-0000-4000-8000-000000000012",
            decision_id,
            "00000000-0000-0000-0000-000000000000",
        ),
        "23514",
        Some("c4_delivery_consignment_payload_ck"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_release_hold_stays_held_for_review() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('c0000000-0000-4000-8000-000000000001', 2, 'admin', 'fake-admin',
                     'admin_delist', '{LISTING}', decode(repeat('c6', 32), 'hex'), 'listing',
                     '{LISTING}', '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",
                        \"command_id\":\"c0000000-0000-4000-8000-000000000001\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                        \"state\":\"held_off_market\",
                        \"held_reason\":\"operations_hold\",
                        \"hold_id\":\"c1000000-0000-4000-8000-000000000001\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, 'c0000000-0000-4000-8000-000000000001', 'admin',
                     'hold_set', 'deposit_accepted', 'held_off_market', 'operations_hold',
                     101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=5,
                 updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';
             INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('c0000000-0000-4000-8000-000000000002', 2, 'admin', 'fake-admin',
                     'release_hold', '{LISTING}', decode(repeat('c7', 32), 'hex'), 'listing',
                     '{LISTING}', '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_hold\",
                        \"disposition\":\"hold_released\",
                        \"command_id\":\"c0000000-0000-4000-8000-000000000002\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":6,
                        \"state\":\"held_off_market\",
                        \"held_reason\":\"review\",
                        \"hold_id\":\"c1000000-0000-4000-8000-000000000001\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, 'c0000000-0000-4000-8000-000000000002', 'admin',
                     'hold_released', 'held_off_market', 'held_off_market', 'review',
                     101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET reason_code='review', state_version=6, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
    )
    .await;
    let listing: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT state, reason_code, state_version FROM public.consignment_listings
         WHERE id=$1::uuid",
    )
    .bind(LISTING)
    .fetch_one(pool)
    .await
    .expect("读取解除止售后的挂单");
    assert_eq!(listing, ("held_off_market".to_owned(), Some("review".to_owned()), 6));
    check(
        pool,
        &format!(
            "UPDATE public.consignment_listings
             SET state='available', reason_code=NULL, state_version=7,
                 updated_at='2026-01-01T04:00:00Z'
             WHERE id='{LISTING}';"
        ),
        // 重列边在 000137 放开：没有同事务的重列命令与事件时仍由事件核对拒绝。
        "consignment_listing_version_event",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_schema_rejects_representative_named_constraints() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;

    check(
        pool,
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ('8d000000-0000-0000-0000-000000000002', '2026-2', 'main', 'UTC',
                 '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z',
                 '2026-01-01T12:00:00Z', decode(repeat('79', 32), 'hex'), 3);",
        "c4_season_world_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.credential_keys
                (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
                 fingerprint, status, state_version, created_at, updated_at)
             VALUES ('b2000000-0000-4000-8000-000000000001', 'external_api', 'main',
                     'fake-external-api', decode('11', 'hex'), decode(repeat('22', 12), 'hex'),
                     'fake-data-key', repeat('c', 64), 'active', 1, '{START}', '{START}');
             INSERT INTO public.mail_evidence_sessions
                (id, bot_uid, world_id, player_name, credential_id, credential_version,
                 runtime_generation, observed_at, protocol_version, binding_digest)
             VALUES ('b2100000-0000-4000-8000-000000000001', '12347', 'main', 'FakeBot',
                     'b2000000-0000-4000-8000-000000000001', 1, 1, '{LATER}', 'v1',
                     decode(repeat('c8', 32), 'hex'));"
        ),
        "c4_session_credential",
    )
    .await;
    succeeds(
        owner.postgres_pool(),
        "ALTER TABLE public.consignment_deposit_reservations
         DISABLE TRIGGER c4_res_transition;",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             SELECT '94000000-0000-0000-0000-000000000002', listing_id, seller_user_id,
                    verified_sender, bot_uid, world_id, canonical_eid, season_window_id,
                    generation, service_epoch, session_id, decode(repeat('82', 32), 'hex'),
                    issued_at, '{END}', 'unknown', last_command_id, NULL, NULL
             FROM public.consignment_deposit_reservations WHERE id='{RESERVATION}';"
        ),
        "c4_res_state_ck",
    )
    .await;
    succeeds(
        owner.postgres_pool(),
        "ALTER TABLE public.consignment_deposit_reservations
         ENABLE TRIGGER c4_res_transition;",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_equipment_claims
                (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
                 last_fact_id)
             VALUES ('main', '42', 2, '{RESERVATION}', '{POSITION}', '{ACCEPTED}',
                     '{POSITION}');"
        ),
        "c4_claim_holder_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_deposit_items
                (id, bot_uid, world_id, external_mid, decision_id, ordinal, canonical_eid,
                 reservation_id, listing_id, seller_user_id, equipment_id,
                 accepted_listing_version, accepted_price, credential_ref, observation_id)
             SELECT '95000000-0000-0000-0000-000000000001', bot_uid, world_id, external_mid,
                    decision_id, ordinal, canonical_eid, reservation_id, listing_id,
                    seller_user_id, equipment_id, accepted_listing_version, 0,
                    credential_ref, observation_id
             FROM public.consignment_deposit_items WHERE id='{ITEM}';"
        ),
        "c4_item_versions_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.consignment_custody_events
                (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
                 listing_id, listing_version, occurred_at)
             VALUES ('{POSITION}', 2, '41000000-0000-5000-8000-000000000002', 'in_custody',
                     'awaiting_take', 'bogus', NULL, '{LISTING}', 4, '{ACCEPTED}');"
        ),
        "c4_custody_event_proof_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_disposition_work
                (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
                 delivery_id, expected_observation_id, manifest_digest, created_at)
             VALUES ('96000000-0000-0000-0000-000000000001', NULL, NULL, NULL, NULL, NULL,
                     'take', '8a000000-0000-0000-0000-000000000001', NULL, NULL,
                     '{ACCEPTED}');"
        ),
        "c4_work_shape_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('8a000000-0000-0000-0000-000000000003',
                     '8b000000-0000-0000-0000-000000000003',
                     decode(repeat('82', 32), 'hex'), 'consignment_disposition',
                     'consignment:{DECISION}:{WORK}', 'FakeBot',
                     '{{\"schema_version\":1,\"operation\":\"take\",
                        \"world\":\"main\",\"external_mail_id\":\"555\",
                        \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"main\",
                        \"external_mid\":\"555\"}},
                        \"decision_id\":\"{DECISION}\",\"work_id\":\"{WORK}\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');"
        ),
        "c4_delivery_consignment_payload_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('8a000000-0000-0000-0000-000000000004',
                     '8b000000-0000-0000-0000-000000000004',
                     decode(repeat('83', 32), 'hex'), 'consignment_disposition',
                     'consignment:{DECISION}:{WORK}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"take\",
                        \"world\":\"main\",\"external_mail_id\":\"555\",
                        \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"isekai\",
                        \"external_mid\":\"555\"}},
                        \"decision_id\":\"{DECISION}\",\"work_id\":\"{WORK}\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');"
        ),
        "c4_delivery_consignment_payload_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('8a000000-0000-0000-0000-000000000005',
                     '8b000000-0000-0000-0000-000000000005',
                     decode(repeat('84', 32), 'hex'), 'consignment_disposition',
                     'consignment:{DECISION}:{WORK}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"take\",
                        \"world\":\"main\",\"external_mail_id\":\"555\",
                        \"mail_key\":{{\"bot_uid\":\"12345\",\"world_id\":\"main\",
                        \"external_mid\":\"556\"}},
                        \"decision_id\":\"{DECISION}\",\"work_id\":\"{WORK}\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');"
        ),
        "c4_delivery_consignment_payload_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.deliveries
                (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                 status, fencing_version, attempt_count, created_at, updated_at)
             VALUES ('8a000000-0000-0000-0000-000000000002',
                     '8b000000-0000-0000-0000-000000000002',
                     decode(repeat('81', 32), 'hex'), 'consignment_disposition',
                     '{DECISION}', 'FakeBot',
                     '{{\"schema_version\":2,\"operation\":\"take\"}}'::jsonb,
                     'prepared', 0, 0, '{ACCEPTED}', '{ACCEPTED}');"
        ),
        "c4_delivery_consignment_payload_ck",
    )
    .await;
    check(
        pool,
        &format!(
            "INSERT INTO public.mail_final_dispositions
                (bot_uid, world_id, external_mid, id, kind, source_observation_id,
                 command_id, decided_at)
             VALUES ('12345', 'main', '555', '97000000-0000-0000-0000-000000000001',
                     'acquire_return', '{OBSERVATION}', '{DECISION}', '{ACCEPTED}');"
        ),
        // 首次决定是 accepted：acquire_return 终局与其种类不符，先被 c4_final_initial 拒绝；
        // 没有 intake 的 acquire_return 由下一个用例覆盖。
        "c4_final_initial",
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_facts_reject_mutation_and_projection_updates_are_scoped() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    succeeds(pool, &controls_notice_and_handoff()).await;
    succeeds(pool, &quarantine_release_fixture()).await;
    seed_settlement_recovery_handoff(pool).await;

    for (table, column) in FACT_UPDATE_COLUMNS {
        let update = format!(
            "UPDATE public.{table} SET {column}={column}
             WHERE ctid IN (SELECT ctid FROM public.{table} LIMIT 1)"
        );
        assert!(execute(pool, &update).await.is_err(), "{update} 应被拒绝");
        let delete = format!(
            "DELETE FROM public.{table}
             WHERE ctid IN (SELECT ctid FROM public.{table} LIMIT 1)"
        );
        assert!(execute(pool, &delete).await.is_err(), "{delete} 应被拒绝");
        assert!(
            execute(pool, &format!("TRUNCATE public.{table} CASCADE")).await.is_err(),
            "{table} 的 TRUNCATE 应被拒绝"
        );
    }
    fails(
        owner.postgres_pool(),
        "UPDATE public.consignment_deposit_items SET ordinal=ordinal",
        "55000",
        None,
    )
    .await;
    fails(owner.postgres_pool(), "DELETE FROM public.mail_final_dispositions", "55000", None).await;

    succeeds(pool, "UPDATE public.consignment_custody_positions SET state=state WHERE false").await;
    fails(
        pool,
        "UPDATE public.consignment_custody_positions SET item_id=item_id WHERE false",
        "42501",
        None,
    )
    .await;
    succeeds(pool, "UPDATE public.consignment_deposit_reservations SET state=state WHERE false")
        .await;
    fails(
        pool,
        "UPDATE public.consignment_deposit_reservations SET token_digest=token_digest WHERE false",
        "42501",
        None,
    )
    .await;
    fails(pool, "UPDATE public.mail_final_dispositions SET kind=kind WHERE false", "42501", None)
        .await;
}

#[tokio::test]
async fn postgres_consignment_deposit_acquire_return_requires_real_intake() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &additional_rows()).await;

    check(
        pool,
        "INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12346', 'main', '556', '98000000-0000-0000-0000-000000000001',
                 'acquire_return', '8f000000-0000-0000-0000-000000000001',
                 '92000000-0000-4000-8000-000000000001', '2026-01-01T04:00:00Z');",
        "c4_acquire_return_intake_required",
    )
    .await;
    // 解除表上的 acquire_return 同样由延迟触发器 c4_reject_acquire_return_without_intake 要求同事务 intake；
    // 本夹具没有可通过 c4_release_quarantine_fk 的隔离行，解除表分支未在此覆盖。
}
