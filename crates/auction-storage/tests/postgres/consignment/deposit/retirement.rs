//! 预约退休/消费的命令绑定：无关命令、原因/目标不符、换发后继与整封消费事实。

use sqlx::{AssertSqlSafe, PgPool};

use super::lot_guard::{insert_equipment, seed_auction};
use super::support::{
    ACCEPTED, DECIDE, DECISION, END, EXPIRE_COMMAND, ISSUE, ISSUED, LISTING, POSITION,
    REISSUE_COMMAND, RESERVATION, SESSION, SNAPSHOT, START, USER_1, accept_listing_as_with_actor,
    custody_event, decide_command, delivery_and_work, expire_reservation_command, fails,
    initial_and_final, item_position_claim, listing_awaiting_deposit, mail_session,
    observation_and_asset, reissue_reservation_command, reservation_issued, succeeds,
    users_and_key,
};
use crate::support::TestDatabase;

const SECOND_LISTING: &str = "20000000-0000-0000-0000-000000000002";
const SECOND_RESERVATION: &str = "82000000-0000-0000-0000-000000000002";
const SECOND_SNAPSHOT: &str = "30000000-0000-0000-0000-000000000002";
const OTHER_EXPIRE_COMMAND: &str = "41000000-0000-5000-8000-000000000009";
const WITHDRAW_COMMAND: &str = "41000000-0000-4000-8000-000000000005";
const ADMIN_DELIST_COMMAND: &str = "41000000-0000-4000-8000-000000000006";
const V1_WITHDRAW_COMMAND: &str = "41000000-0000-4000-8000-000000000007";
const HOLD_ACCEPT_COMMAND: &str = "41000000-0000-5000-8000-00000000000a";
const HOLD_COMMAND: &str = "41000000-0000-4000-8000-00000000000b";
const HOLD_ID: &str = "a1000000-0000-4000-8000-00000000000b";
const REISSUE_SUCCESSOR: &str = "82000000-0000-0000-0000-000000000003";
const REISSUE_DECOY: &str = "82000000-0000-0000-0000-000000000004";
const RELEASE_COMMAND: &str = "9c00000a-0000-4000-8000-000000000002";
const RELEASE_ID: &str = "9f000000-0000-4000-8000-000000000002";
const QUARANTINE: &str = "91000000-0000-0000-0000-000000000002";
const QUARANTINE_OBSERVATION: &str = "8f000000-0000-0000-0000-000000000002";
const LOT: &str = "62000000-0000-0000-0000-000000000020";
const EQUIPMENT: &str = "61000000-0000-0000-0000-000000000020";

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
        .fetch_one(pool)
        .await
        .expect("读取表计数")
}

async fn lot_exists(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM public.lots WHERE id=$1::uuid)")
        .bind(LOT)
        .fetch_one(pool)
        .await
        .expect("读取 lot")
}

fn claim_holding_reservation() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    )
}

fn lot_sql(auction: &str) -> String {
    format!(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price,
             category_key, category_number)
         VALUES ('{LOT}', '{auction}', '{EQUIPMENT}', 20, 'unsold', 100, 'Weapon', 20);"
    )
}

fn release_claim(command_id: &str, reason: &str, auction: &str) -> String {
    format!(
        "UPDATE public.consignment_deposit_reservations
         SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='{reason}',
             last_command_id='{command_id}'
         WHERE id='{RESERVATION}';
         UPDATE public.consignment_equipment_claims
         SET reservation_id=NULL, position_id=NULL, revision=revision+1,
             changed_at='2026-01-01T04:00:00Z', last_fact_id='{RESERVATION}'
         WHERE world_id='main' AND canonical_eid='42';
         {}",
        lot_sql(auction)
    )
}

fn second_listing() -> String {
    format!(
        "ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
         INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch,
             blacklist_count_at_register, blacklist_baseline_ids, current_snapshot_id,
             reason_code, created_at, updated_at, expires_at, registration_xact_id)
         VALUES ('{SECOND_LISTING}', '{USER_1}', 'main', '43', 'consigned',
                 'awaiting_deposit', 101, 1, 500, 2, 1, 0, ARRAY[]::uuid[],
                 '{SECOND_SNAPSHOT}', NULL, '{START}', '{START}', '{END}',
                 pg_current_xact_id());
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         SELECT '{SECOND_SNAPSHOT}', '{SECOND_LISTING}', generation, adapter, observed_at,
                decode(repeat('7a', 32), 'hex'), world_id, '43', name, category,
                alt_name, level_kind, level_number, tradeable, owner_name, stat_groups
         FROM public.consignment_snapshots WHERE id='{SNAPSHOT}';"
    )
}

fn second_reservation() -> String {
    format!(
        "ALTER TABLE public.consignment_listings ENABLE TRIGGER USER;
         INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
             canonical_eid, season_window_id, generation, service_epoch, session_id,
             token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
         VALUES ('{SECOND_RESERVATION}', '{SECOND_LISTING}', '{USER_1}', 'FakeSellerOne',
                 '12345', 'main', '43', NULL, 1, 1, '{SESSION}',
                 decode(repeat('71', 32), 'hex'), '{ISSUED}',
                 '2026-01-01T12:00:00Z', 'active', '{ISSUE}', NULL, NULL);"
    )
}

fn unrelated_expire_command() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{OTHER_EXPIRE_COMMAND}', 2, 'worker', 'fake-worker',
                 'expire_reservation', '{SECOND_RESERVATION}',
                 decode(repeat('72', 32), 'hex'), 'reservation',
                 '{SECOND_RESERVATION}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"expire_reservation\",
                    \"disposition\":\"reservation_retired\",\"command_id\":
                    \"{OTHER_EXPIRE_COMMAND}\",\"reservation_id\":
                    \"{SECOND_RESERVATION}\"}}', '{ISSUED}');"
    )
}

fn withdraw_command() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{WITHDRAW_COMMAND}', 2, 'public_user', '{USER_1}', 'withdraw',
                 '{LISTING}', decode(repeat('73', 32), 'hex'), 'listing', '{LISTING}',
                 '{LISTING}', 202, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"withdraw\",
                    \"disposition\":\"withdrawal_requested\",\"command_id\":
                    \"{WITHDRAW_COMMAND}\",\"listing_id\":\"{LISTING}\",
                    \"listing_version\":4,\"state\":\"deposit_accepted\",
                    \"intent_id\":\"a0000000-0000-4000-8000-000000000009\"}}',
                 '{ISSUED}');"
    )
}

fn admin_delist_command_and_listing_event() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, replay_key_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{ADMIN_DELIST_COMMAND}', 1, 'admin', 'fake-admin', 'admin_delist',
                 '{LISTING}', decode(repeat('74', 32), 'hex'), NULL, '{LISTING}', 200, 'ok',
                 '{{\"command_id\":\"{ADMIN_DELIST_COMMAND}\",
                    \"operation\":\"admin_delist\",\"listing_id\":\"{LISTING}\",
                    \"listing_version\":4,\"state\":\"closed\",
                    \"disposition\":\"closed\",\"price\":101}}', '{ISSUED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 4, '{ADMIN_DELIST_COMMAND}', 'admin', 'closed',
                 'awaiting_deposit', 'closed', 'admin_delisted', 101, '{ISSUED}');
         UPDATE public.consignment_listings
         SET state='closed', reason_code='admin_delisted', state_version=4,
             updated_at='{ISSUED}'
         WHERE id='{LISTING}';"
    )
}

fn v1_withdraw_command_and_listing_event() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, replay_key_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{V1_WITHDRAW_COMMAND}', 1, 'public_user', '{USER_1}', 'withdraw',
                 '{LISTING}', decode(repeat('7b', 32), 'hex'), NULL, '{LISTING}', 200, 'ok',
                 '{{\"command_id\":\"{V1_WITHDRAW_COMMAND}\",
                    \"operation\":\"withdraw\",\"listing_id\":\"{LISTING}\",
                    \"listing_version\":4,\"state\":\"closed\",
                    \"disposition\":\"closed\",\"price\":101}}', '{ISSUED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 4, '{V1_WITHDRAW_COMMAND}', 'public_user', 'closed',
                 'awaiting_deposit', 'closed', 'seller_cancelled', 101, '{ISSUED}');
         UPDATE public.consignment_listings
         SET state='closed', reason_code='seller_cancelled', state_version=4,
             updated_at='{ISSUED}'
         WHERE id='{LISTING}';"
    )
}

// 把挂单推进到 deposit_accepted 但保留 active 预约，作为 v2 止售命令的夹具。
// 本测试只验证预约退休的命令绑定，不构造完整整封接受图。
fn accepted_listing_for_hold() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_bot_uid, target_world_id, target_external_mid,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{HOLD_ACCEPT_COMMAND}', 2, 'worker', 'fake-worker', 'decide_envelope',
                 '12345|main|559', decode(repeat('7c', 32), 'hex'), 'mail_key', '12345',
                 'main', '559', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"decide_envelope\",
                    \"disposition\":\"accepted\",\"command_id\":\"{HOLD_ACCEPT_COMMAND}\",
                    \"decision_id\":\"87000000-0000-0000-0000-000000000009\",
                    \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",
                    \"listing_version\":4,\"state\":\"deposit_accepted\"}}],
                    \"quarantine\":null}}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 4, '{HOLD_ACCEPT_COMMAND}', 'worker', 'deposit_accepted',
                 'awaiting_deposit', 'deposit_accepted', NULL, 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state='deposit_accepted', state_version=4, updated_at='{ACCEPTED}'
         WHERE id='{LISTING}';"
    )
}

fn hold_set_command_and_listing_event() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{HOLD_COMMAND}', 2, 'admin', 'fake-admin', 'admin_delist',
                 '{LISTING}', decode(repeat('7d', 32), 'hex'), 'listing', '{LISTING}',
                 '{LISTING}', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                    \"disposition\":\"hold_set\",\"command_id\":\"{HOLD_COMMAND}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                    \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                    \"hold_id\":\"{HOLD_ID}\"}}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 5, '{HOLD_COMMAND}', 'admin', 'hold_set',
                 'deposit_accepted', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state='held_off_market', reason_code='operations_hold', state_version=5,
             updated_at='{ACCEPTED}'
         WHERE id='{LISTING}';"
    )
}

fn listing_version_four_event(command_id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', 4, '{command_id}', 'public_user', 'reservation_issued',
                 'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
         UPDATE public.consignment_listings SET state_version=4, updated_at='{ISSUED}'
         WHERE id='{LISTING}';"
    )
}

fn successor_reservation() -> String {
    format!(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
             canonical_eid, season_window_id, generation, service_epoch, session_id,
             token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
         VALUES ('{REISSUE_SUCCESSOR}', '{LISTING}', '{USER_1}', 'FakeSellerOne', '12345',
                 'main', '42', NULL, 2, 1, '{SESSION}', decode(repeat('75', 32), 'hex'),
                 '{ISSUED}', '2026-01-01T12:00:00Z', 'active', '{REISSUE_COMMAND}',
                 NULL, NULL);"
    )
}

fn release_command() -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{RELEASE_COMMAND}', 2, 'admin', 'fake-admin', 'release_quarantine',
                 '{QUARANTINE}', decode(repeat('76', 32), 'hex'), 'quarantine',
                 '{QUARANTINE}', NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                    \"disposition\":\"quarantine_released\",\"command_id\":
                    \"{RELEASE_COMMAND}\",\"quarantine_id\":\"{QUARANTINE}\",
                    \"release_id\":\"{RELEASE_ID}\",\"decision_id\":\"{DECISION}\",
                    \"decision_kind\":\"accept\",\"listing_changes\":[
                      {{\"listing_id\":\"{LISTING}\",\"listing_version\":4,
                        \"state\":\"deposit_accepted\"}}]}}', '{ACCEPTED}');"
    )
}

fn quarantine_and_release_rows() -> String {
    format!(
        "INSERT INTO public.mail_quarantines
            (id, bot_uid, world_id, external_mid, initial_decision_id, version, state,
             reread_locator, responsibility_code, created_at, updated_at)
         VALUES ('{QUARANTINE}', '12345', 'main', '555',
                 '86000000-0000-0000-0000-000000000001', 1, 'active',
                 'controlled-reread', 'identity_unproven', '{ACCEPTED}', '{ACCEPTED}');
         INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ('{QUARANTINE_OBSERVATION}', '12345', 'main', '555', 'obs-release',
                 '{SESSION}', 'fake-mailbox', '{QUARANTINE}', 1, '{ACCEPTED}',
                 'parser-v1', NULL, decode(repeat('77', 32), 'hex'),
                 decode(repeat('78', 32), 'hex'), decode(repeat('79', 32), 'hex'),
                 'FakeSellerOne', 'FakeBot', true, true, 0, 0, NULL, '[]'::jsonb,
                 '{{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T03:00\",\"items\":[]}}'::jsonb,
                 NULL);
         INSERT INTO public.mail_quarantine_releases
            (id, bot_uid, world_id, external_mid, quarantine_id, expected_version,
             observation_id, decision_id, kind, command_id, case_return_mail_id,
             case_evidence_id, recovery_handoff_id, released_at)
         VALUES ('{RELEASE_ID}', '12345', 'main', '555', '{QUARANTINE}', 1,
                 '{QUARANTINE_OBSERVATION}', '{DECISION}', 'accepted',
                 '{RELEASE_COMMAND}', NULL, NULL, NULL, '{ACCEPTED}');"
    )
}

fn consumed_release() -> String {
    format!(
        "UPDATE public.consignment_deposit_reservations
         SET state='consumed', last_command_id='{RELEASE_COMMAND}'
         WHERE id='{RESERVATION}';"
    )
}

async fn seed_release_fixture(pool: &PgPool) {
    let graph = users_and_key()
        + &listing_awaiting_deposit()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &release_command()
        + &initial_and_final()
            .replace(
                &format!("'accepted', NULL, '{ACCEPTED}');"),
                &format!("'quarantined', '{QUARANTINE}', '{ACCEPTED}');"),
            )
            .replace(
                &format!("'{DECIDE}', '{ACCEPTED}');"),
                &format!("'{RELEASE_COMMAND}', '{ACCEPTED}');"),
            )
        + &quarantine_and_release_rows()
        + &item_position_claim()
        + &consumed_release()
        + &accept_listing_as_with_actor(RELEASE_COMMAND, "admin")
        + &delivery_and_work()
        + &custody_event();
    succeeds(pool, &graph).await;
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_rejects_expire_for_another_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner_pool = owner.postgres_pool();
    succeeds(
        owner_pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    succeeds(owner_pool, &second_listing()).await;
    succeeds(owner_pool, &second_reservation()).await;
    let auction = seed_auction(owner_pool, 20).await;
    insert_equipment(owner_pool, EQUIPMENT, "42").await;

    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let commands_before = count(pool, "consignment_commands").await;
    let events_before = count(pool, "consignment_events").await;
    fails(
        pool,
        &format!(
            "{}{}",
            unrelated_expire_command(),
            release_claim(OTHER_EXPIRE_COMMAND, "cancelled", &auction)
        ),
        "23514",
        Some("c4_res_transition_fact"),
    )
    .await;
    assert_eq!(count(pool, "consignment_commands").await, commands_before);
    assert_eq!(count(pool, "consignment_events").await, events_before);
    assert!(!lot_exists(pool).await, "无关命令不得为实物释放背书");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));

    let owner = database.maintenance_store().await.expect("以维护账号连接");
    fails(
        owner.postgres_pool(),
        &format!(
            "ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition;
             ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition_fact;
             {}{}",
            unrelated_expire_command(),
            release_claim(OTHER_EXPIRE_COMMAND, "cancelled", &auction)
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert!(!lot_exists(owner.postgres_pool()).await, "claim 复检也不得留下 lot");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(owner.postgres_pool())
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_rejects_reissue_as_cancellation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 21).await;
    insert_equipment(pool, EQUIPMENT, "42").await;

    let commands_before = count(pool, "consignment_commands").await;
    let events_before = count(pool, "consignment_events").await;
    fails(
        pool,
        &format!(
            "{}{}{}",
            reissue_reservation_command(RESERVATION),
            listing_version_four_event(REISSUE_COMMAND),
            release_claim(REISSUE_COMMAND, "cancelled", &auction)
        ),
        "23514",
        Some("c4_res_transition_fact"),
    )
    .await;
    assert_eq!(count(pool, "consignment_commands").await, commands_before);
    assert_eq!(count(pool, "consignment_events").await, events_before);
    assert!(!lot_exists(pool).await, "换发命令不得为取消释放背书");

    let owner = database.maintenance_store().await.expect("以维护账号连接");
    fails(
        owner.postgres_pool(),
        &format!(
            "ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition;
             ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition_fact;
             {}{}{}",
            reissue_reservation_command(RESERVATION),
            listing_version_four_event(REISSUE_COMMAND),
            release_claim(REISSUE_COMMAND, "cancelled", &auction)
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert!(!lot_exists(owner.postgres_pool()).await, "claim 复检也不得留下 lot");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(owner.postgres_pool())
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_expire_for_each_reason() {
    for reason in ["expired", "identity_lost", "service_disabled"] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("以运行账号连接");
        let pool = runtime.postgres_pool();
        succeeds(
            pool,
            &(users_and_key()
                + &listing_awaiting_deposit()
                + &mail_session()
                + &reservation_issued()
                + &claim_holding_reservation()),
        )
        .await;
        let auction = seed_auction(pool, 22).await;
        insert_equipment(pool, EQUIPMENT, "42").await;
        succeeds(
            pool,
            &(expire_reservation_command() + &release_claim(EXPIRE_COMMAND, reason, &auction)),
        )
        .await;
        assert!(lot_exists(pool).await, "{reason} 的真实退休应允许 lot 提交");
    }
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_rejects_expire_as_cancellation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 26).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    fails(
        pool,
        &(expire_reservation_command() + &release_claim(EXPIRE_COMMAND, "cancelled", &auction)),
        "23514",
        Some("c4_res_transition_fact"),
    )
    .await;
    assert!(!lot_exists(pool).await, "卖家取消不能由 expire_reservation 单独背书");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
    let state: String = sqlx::query_scalar(
        "SELECT state FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(RESERVATION)
    .fetch_one(pool)
    .await
    .expect("读取预约状态");
    assert_eq!(state, "active");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_v1_admin_delist_cancellation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 27).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(
        pool,
        &(admin_delist_command_and_listing_event()
            + &release_claim(ADMIN_DELIST_COMMAND, "cancelled", &auction)),
    )
    .await;
    assert!(lot_exists(pool).await, "v1 admin_delist 关闭应允许取消预约退休");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_v2_hold_set_cancellation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    succeeds(pool, &accepted_listing_for_hold()).await;
    let auction = seed_auction(pool, 28).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(
        pool,
        &(hold_set_command_and_listing_event()
            + &release_claim(HOLD_COMMAND, "cancelled", &auction)),
    )
    .await;
    assert!(lot_exists(pool).await, "v2 止售应允许取消预约退休");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_v1_admin_delist_identity_lost() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 29).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(
        pool,
        &(admin_delist_command_and_listing_event()
            + &release_claim(ADMIN_DELIST_COMMAND, "identity_lost", &auction)),
    )
    .await;
    assert!(lot_exists(pool).await, "v1 admin_delist 应允许身份失效预约退休");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_v1_withdraw_cancellation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 30).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(
        pool,
        &(v1_withdraw_command_and_listing_event()
            + &release_claim(V1_WITHDRAW_COMMAND, "cancelled", &auction)),
    )
    .await;
    assert!(lot_exists(pool).await, "v1 withdraw 关闭应允许取消预约退休");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_rejects_a_prior_transaction_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()
            + &expire_reservation_command()),
    )
    .await;
    let auction = seed_auction(pool, 25).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    fails(
        pool,
        &release_claim(EXPIRE_COMMAND, "cancelled", &auction),
        "23514",
        Some("c4_res_transition_fact"),
    )
    .await;
    assert!(!lot_exists(pool).await, "先前事务的命令不得为本次释放背书");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));

    let owner = database.maintenance_store().await.expect("以维护账号连接");
    fails(
        owner.postgres_pool(),
        &format!(
            "ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition;
             ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition_fact;
             {}",
            release_claim(EXPIRE_COMMAND, "cancelled", &auction)
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert!(!lot_exists(owner.postgres_pool()).await, "claim 复检也不得留下 lot");
}

#[tokio::test]
async fn postgres_consignment_reservation_retirement_accepts_listing_commands() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 23).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(pool, &(withdraw_command() + &release_claim(WITHDRAW_COMMAND, "cancelled", &auction)))
        .await;
    assert!(lot_exists(pool).await, "卖家取消挂单应允许其预约退休");

    let second = TestDatabase::create().await.expect("克隆第二个模板库");
    let runtime = second.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let auction = seed_auction(pool, 24).await;
    insert_equipment(pool, EQUIPMENT, "42").await;
    succeeds(
        pool,
        &(admin_delist_command_and_listing_event()
            + &release_claim(ADMIN_DELIST_COMMAND, "service_disabled", &auction)),
    )
    .await;
    assert!(lot_exists(pool).await, "Admin 停用下架应允许其预约退休");
}

#[tokio::test]
async fn postgres_consignment_reservation_reissue_requires_successor_generation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    succeeds(
        pool,
        &format!(
            "{}{}
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='reissued',
                 last_command_id='{REISSUE_COMMAND}'
             WHERE id='{RESERVATION}';
             {}
             UPDATE public.consignment_equipment_claims
             SET reservation_id='{REISSUE_SUCCESSOR}', revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{REISSUE_SUCCESSOR}'
             WHERE world_id='main' AND canonical_eid='42';",
            reissue_reservation_command(REISSUE_SUCCESSOR),
            listing_version_four_event(REISSUE_COMMAND),
            successor_reservation(),
        ),
    )
    .await;
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(REISSUE_SUCCESSOR));
}

#[tokio::test]
async fn postgres_consignment_reservation_reissue_rejects_wrong_successor_generation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    let wrong_successor = successor_reservation()
        .replace(&format!("NULL, 2, 1, '{SESSION}'"), &format!("NULL, 3, 1, '{SESSION}'"));
    fails(
        pool,
        &format!(
            "{}{}
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='reissued',
                 last_command_id='{REISSUE_COMMAND}'
             WHERE id='{RESERVATION}';
             {}
             UPDATE public.consignment_equipment_claims
             SET reservation_id='{REISSUE_SUCCESSOR}', revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{REISSUE_SUCCESSOR}'
             WHERE world_id='main' AND canonical_eid='42';",
            reissue_reservation_command(REISSUE_SUCCESSOR),
            listing_version_four_event(REISSUE_COMMAND),
            wrong_successor,
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_reservation_reissue_rejects_mismatched_receipt() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_holding_reservation()),
    )
    .await;
    fails(
        pool,
        &format!(
            "{}{}
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='reissued',
                 last_command_id='{REISSUE_COMMAND}'
             WHERE id='{RESERVATION}';
             {}
             UPDATE public.consignment_equipment_claims
             SET reservation_id='{REISSUE_SUCCESSOR}', revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{REISSUE_SUCCESSOR}'
             WHERE world_id='main' AND canonical_eid='42';",
            reissue_reservation_command(REISSUE_DECOY),
            listing_version_four_event(REISSUE_COMMAND),
            successor_reservation(),
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    let successor: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(REISSUE_SUCCESSOR)
    .fetch_one(pool)
    .await
    .expect("读取后继预约");
    assert_eq!(successor, 0, "回执绑定失败不得留下后继预约");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
}

#[tokio::test]
async fn postgres_consignment_reservation_consumed_accepts_release_quarantine() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_release_fixture(pool).await;
    let consumed: String = sqlx::query_scalar(
        "SELECT state FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(RESERVATION)
    .fetch_one(pool)
    .await
    .expect("读取预约状态");
    assert_eq!(consumed, "consumed");
    assert_eq!(count(pool, "consignment_deposit_items").await, 1);
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT position_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(POSITION));
}
