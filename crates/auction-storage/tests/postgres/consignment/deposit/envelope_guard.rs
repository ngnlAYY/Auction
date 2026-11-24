//! 整封接受、拒退、空信与旧 owner 交接的提交前守卫。

use super::support::{
    ACCEPTED, END, ISSUED, LISTING, RESERVATION, SESSION, START, USER_1, USER_2, accept_listing,
    accepted_envelope, consume_reservation, custody_event, decide_command, delivery_and_work,
    fails, initial_and_final, item_position_claim, listing_awaiting_deposit, mail_session,
    observation_and_asset, reservation_issued, seed_accepted, succeeds, users_and_key,
};
use crate::support::TestDatabase;

async fn count(pool: &sqlx::PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
        .fetch_one(pool)
        .await
        .expect("读取计数")
}

async fn assert_rolled_back(pool: &sqlx::PgPool) {
    for table in [
        "consignment_deposit_items",
        "consignment_custody_positions",
        "mail_disposition_work",
        "mail_final_dispositions",
        "mail_initial_decisions",
        "consignment_equipment_claims",
    ] {
        assert_eq!(count(pool, table).await, 0, "{table} 应随提交失败回滚");
    }
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_commits_one_to_one() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    assert_eq!(count(pool, "consignment_deposit_items").await, 1);
    assert_eq!(count(pool, "consignment_custody_positions").await, 1);
    assert_eq!(count(pool, "mail_disposition_work").await, 1);
    let consumed: String = sqlx::query_scalar(
        "SELECT state FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(RESERVATION)
    .fetch_one(pool)
    .await
    .expect("读取预约状态");
    assert_eq!(consumed, "consumed");
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_missing_item_rolls_back() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let graph =
        accepted_envelope().replace(&item_position_claim(), "").replace(&custody_event(), "");
    fails(pool, &graph, "23514", Some("c4_envelope_accept_counts")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_raw_count_mismatch_rolls_back() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let envelope = accepted_envelope();
    let graph = envelope.replace("true, true, 0, 0, 1,", "true, true, 0, 0, 2,");
    assert_ne!(graph, envelope, "观察 raw_count 必须被改成 2");
    fails(pool, &graph, "23514", Some("c4_envelope_accept_counts")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_missing_take_rolls_back() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let graph = accepted_envelope().replace(&delivery_and_work(), "");
    fails(pool, &graph, "23514", Some("c4_envelope_accept_counts")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_unconsumed_reservation_rolls_back() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let graph = accepted_envelope().replace(&consume_reservation(), "");
    fails(pool, &graph, "23514", Some("c4_envelope_accept_bindings")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_missing_listing_event_rolls_back() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let accept = accept_listing();
    let update = accept
        .split_once("UPDATE public.consignment_listings")
        .map(|(_, rest)| format!("UPDATE public.consignment_listings{rest}"))
        .expect("接受夹具包含挂单更新");
    let graph = accepted_envelope().replace(&accept, &update);
    fails(pool, &graph, "23514", Some("c4_envelope_accept_bindings")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_accept_reservation_seller_mismatch_rejected_by_fk() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    // 第二张同物挂单属于 USER_2，使 item 的 listing 外键成立，只有预约卖家不一致。
    let second_listing = "20000000-0000-0000-0000-000000000002";
    let listing_two = format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{second_listing}', '{USER_2}', 'main', '42', 'consigned', 'resolving', 101,
                 1, 500, 1, 1, 0, ARRAY[]::uuid[], '{START}', '{START}', '{END}');"
    );
    let item = item_position_claim().replace(
        &format!("'{LISTING}', '{USER_1}', "),
        &format!("'{second_listing}', '{USER_2}', "),
    );
    let graph = accepted_envelope()
        .replace(&listing_awaiting_deposit(), &(listing_awaiting_deposit() + &listing_two))
        .replace(&item_position_claim(), &item);
    fails(pool, &graph, "23503", Some("c4_item_reservation_fk")).await;
    assert_rolled_back(pool).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_return_requires_exactly_one_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let return_work =
        delivery_and_work().replace("'take'", "'return'").replace("\"take\"", "\"return\"");
    let mut graph = users_and_key()
        + &listing_awaiting_deposit()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &initial_and_final().replace("'accepted'", "'return_decided'")
        + &return_work;
    graph = graph.replace(&item_position_claim(), "").replace(&accept_listing(), "");
    succeeds(pool, &graph).await;
    assert_eq!(count(pool, "consignment_deposit_items").await, 0);
    assert_eq!(count(pool, "mail_disposition_work").await, 1);

    let second = TestDatabase::create().await.expect("克隆第二个模板库");
    let second_runtime = second.store().await.expect("以运行账号连接");
    let second_pool = second_runtime.postgres_pool();
    let missing = graph.replace(&return_work, "");
    fails(second_pool, &missing, "23514", Some("c4_envelope_return_shape")).await;
}

#[tokio::test]
async fn postgres_consignment_deposit_empty_archive_has_zero_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let graph = users_and_key()
        + &listing_awaiting_deposit()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &initial_and_final().replace("'accepted'", "'archived_empty'");
    succeeds(pool, &graph).await;
    assert_eq!(count(pool, "mail_disposition_work").await, 0);
}

#[tokio::test]
async fn postgres_consignment_deposit_old_owner_case_has_exactly_one_owner_and_zero_work() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let graph = format!(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES
            ('{USER_1}', 'FakeSellerOne', decode(repeat('11', 32), 'hex'));
         INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
             fingerprint, status, state_version, created_at, updated_at)
         VALUES ('80000000-0000-0000-0000-000000000001', 'mail_cookie', 'main', 'fake',
                 decode('11', 'hex'), decode(repeat('22', 12), 'hex'), 'fake-data-key',
                 repeat('a', 64), 'active', 1, '{START}', '{START}');
         INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ('{SESSION}', '12346', 'main', 'FakeBot',
                 '80000000-0000-0000-0000-000000000001', 1, 1, '{ISSUED}', 'v1',
                 decode(repeat('33', 32), 'hex'));
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('9f000000-0000-4000-8000-000000000001', 'admin', 'fake-admin',
                 'admin_delist', '9f000000-0000-0000-0000-000000000002',
                 decode(repeat('7e', 32), 'hex'), NULL, NULL, 404, 'listing_not_found',
                 '{{\"command_id\":\"9f000000-0000-4000-8000-000000000001\",
                   \"operation\":\"admin_delist\",\"listing_id\":null,
                   \"listing_version\":null,\"state\":null,
                   \"disposition\":\"rejected\",\"price\":null}}', '{ACCEPTED}');
         INSERT INTO public.mail_originals
            (bot_uid, world_id, external_mid, first_session_id, received_minute_raw,
             received_timezone, first_seen_at, season_window_id)
         VALUES ('12346', 'main', '556', '{SESSION}', '2026-01-01T03:00', 'UTC',
                 '{ACCEPTED}', NULL);
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('8f000000-0000-0000-0000-000000000001', '12346', 'main', '556', 'obs-q-1',
                 '{SESSION}', 'fake-mailbox', NULL, NULL, '{ACCEPTED}', 'parser-v1', NULL,
                 decode(repeat('75', 32), 'hex'), decode(repeat('76', 32), 'hex'),
                 decode(repeat('77', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"unknown\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":true,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ('12346', 'main', '556', '90000000-0000-0000-0000-000000000001',
                 '8f000000-0000-0000-0000-000000000001',
                 '9f000000-0000-4000-8000-000000000001', 'quarantined',
                 '91000000-0000-0000-0000-000000000001', '{ACCEPTED}');
         INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ('91000000-0000-0000-0000-000000000001', '12346', 'main', '556',
                 '90000000-0000-0000-0000-000000000001', 1, 'active', 'controlled-reread',
                 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('9a000000-0000-0000-0000-000000000001', '12346', 'main', '556', 'obs-q-2',
                 '{SESSION}', 'fake-mailbox', '91000000-0000-0000-0000-000000000001', 1,
                 '{ACCEPTED}', 'parser-v1', 'controlled-ref',
                 decode(repeat('78', 32), 'hex'), decode(repeat('79', 32), 'hex'),
                 decode(repeat('7a', 32), 'hex'), 'MoogleMail', 'FakeBot',
                 true, true, 0, 0, 1,
                 '[{{\"ordinal\":1,\"kind\":\"unknown\"}}]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":true,
                   \"system_rts\":true,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ('9b000000-0000-0000-0000-000000000001', 'fake-mailbox', '556', 'main',
                 'delivery', decode(repeat('7b', 32), 'hex'), '{ACCEPTED}', 'accepted',
                 'settlement_recovery_returned', '{{}}'::jsonb, '{ACCEPTED}',
                 'MoogleMail', '[RTS] Auction equipment delivery');
         INSERT INTO public.settlement_return_evidence
            (return_mail_id, match_state, reason_code, obligation_id, delivery_id,
             previous_execution_state, recorded_at)
         VALUES ('9b000000-0000-0000-0000-000000000001', 'unbound', 'obligation_unknown',
                 NULL, NULL, NULL, '{ACCEPTED}');
         INSERT INTO public.settlement_return_cases
            (return_mail_id, world_id, obligation_id, delivery_id, dispatch_no, status,
             reason_code, plan, resend_limit, resend_dispatch_no, case_version,
             recheck_claim_token, recheck_claim_expires_at, recheck_count, verified_version,
             created_at, updated_at, closed_at)
         VALUES ('9b000000-0000-0000-0000-000000000001', 'main', NULL, NULL, NULL,
                 'quarantined', 'obligation_unknown', NULL, 0, NULL, 1, NULL, NULL, 0,
                 NULL, '{ACCEPTED}', '{ACCEPTED}', NULL);
         INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('9c00000a-0000-4000-8000-000000000001', 2, 'admin', 'fake-admin',
                 'release_quarantine', '91000000-0000-0000-0000-000000000001',
                 decode(repeat('7c', 32), 'hex'), 'quarantine',
                 '91000000-0000-0000-0000-000000000001', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                    \"disposition\":\"quarantine_released\",\"command_id\":
                    \"9c00000a-0000-4000-8000-000000000001\",
                    \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                    \"release_id\":\"9d000000-0000-0000-0000-000000000001\",
                    \"decision_id\":\"9e000000-0000-0000-0000-000000000001\",
                    \"decision_kind\":\"auction_return_case\",
                    \"return_mail_id\":\"9b000000-0000-0000-0000-000000000001\",
                    \"return_evidence_id\":\"9b000000-0000-0000-0000-000000000001\",
                    \"obligation_id\":null}}', '{ACCEPTED}');
         INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ('12346', 'main', '556', '9e000000-0000-0000-0000-000000000001',
                 'auction_return_case', '9a000000-0000-0000-0000-000000000001',
                 '9c00000a-0000-4000-8000-000000000001', '{ACCEPTED}');
         INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ('9d000000-0000-0000-0000-000000000001', '12346', 'main', '556',
                 '91000000-0000-0000-0000-000000000001', 1,
                 '9a000000-0000-0000-0000-000000000001',
                 '9e000000-0000-0000-0000-000000000001', 'auction_return_case',
                 '9c00000a-0000-4000-8000-000000000001',
                 '9b000000-0000-0000-0000-000000000001',
                 '9b000000-0000-0000-0000-000000000001', NULL, '{ACCEPTED}');"
    );
    succeeds(pool, &graph).await;
    assert_eq!(count(pool, "mail_disposition_work").await, 0);
    assert_eq!(count(pool, "consignment_deposit_items").await, 0);
    assert_eq!(count(pool, "mail_quarantine_releases").await, 1);
}
