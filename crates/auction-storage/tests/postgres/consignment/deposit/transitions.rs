//! 第 4 期 v2 挂单迁移：每类状态/版本变化在真实 PG18 上按存储写入形状完整提交，
//! 并覆盖新增分支对命令、actor、目标挂单与回执字段的拒绝。

use sqlx::PgPool;

use super::support::{
    ACCEPTED, ATTEMPT, DECIDE, DECISION, END, ISSUED, LISTING, POSITION, PROOF, REGISTER, REPLAY,
    RESERVATION, START, USER_1, attempt_start_proof_and_custody, expire_reservation_command, fails,
    listing_awaiting_deposit, mail_session, reservation_issued, seed_accepted, succeeds,
    users_and_key,
};
use crate::support::TestDatabase;

const OTHER_LISTING: &str = "20000000-0000-0000-0000-000000000099";
const OTHER_RESERVATION: &str = "82000000-0000-0000-0000-000000000099";
const OTHER_COMMAND_V4: &str = "41000000-0000-4000-8000-000000000099";
const OTHER_COMMAND_V5: &str = "41000000-0000-5000-8000-000000000099";
const PROOF_AVAILABLE: &str = "41000000-0000-5000-8000-000000000020";
const PROOF_HELD: &str = "41000000-0000-5000-8000-000000000021";
const WITHDRAW_AVAILABLE: &str = "41000000-0000-4000-8000-000000000022";
const WITHDRAW_DEPOSIT: &str = "41000000-0000-4000-8000-000000000023";
const WITHDRAW_HELD: &str = "41000000-0000-4000-8000-000000000024";
const WITHDRAW_CLOSED: &str = "41000000-0000-4000-8000-000000000025";
const ADMIN_HOLD_DEPOSIT: &str = "41000000-0000-4000-8000-000000000026";
const ADMIN_HOLD_AGAIN: &str = "41000000-0000-4000-8000-000000000027";
const RELEASE_HOLD: &str = "41000000-0000-4000-8000-000000000028";
const ADMIN_CLOSED: &str = "41000000-0000-4000-8000-000000000029";
const SEASON_CLOSE: &str = "41000000-0000-5000-8000-00000000002a";
const ADMIN_HOLD_REWRITE: &str = "41000000-0000-4000-8000-000000000030";
const WITHDRAW_REWRITE: &str = "41000000-0000-4000-8000-000000000031";
const ADMIN_HOLD_MISMATCH: &str = "41000000-0000-4000-8000-000000000032";
const RELEASE_NON_REVIEW: &str = "41000000-0000-4000-8000-000000000033";
const RELEASE_MISMATCH: &str = "41000000-0000-4000-8000-000000000034";
const PROOF_BAD_REASON: &str = "41000000-0000-5000-8000-000000000035";
const INTENT: &str = "a0000000-0000-4000-8000-000000000020";
const HOLD_DEPOSIT: &str = "a1000000-0000-4000-8000-000000000020";
const HOLD_AGAIN: &str = "a1000000-0000-4000-8000-000000000021";
const HOLD_REWRITE: &str = "a1000000-0000-4000-8000-000000000030";
const HOLD_MISMATCH: &str = "a1000000-0000-4000-8000-000000000031";
const SEASON_ID: &str = "2026-isekai-1";

fn claim_reservation() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    )
}

fn listing_resolving() -> String {
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
                 'resolving', NULL, 101, '{START}');"
    )
}

async fn listing_state(pool: &PgPool) -> (String, i64) {
    sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1::uuid")
        .bind(LISTING)
        .fetch_one(pool)
        .await
        .expect("读取挂单状态")
}

async fn reservation_state(pool: &PgPool) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(RESERVATION)
    .fetch_one(pool)
    .await
    .expect("读取预约状态")
}

async fn event_count(pool: &PgPool, command_id: &str, event_kind: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_events
         WHERE command_id=$1::uuid AND listing_id=$2::uuid AND event_kind=$3",
    )
    .bind(command_id)
    .bind(LISTING)
    .bind(event_kind)
    .fetch_one(pool)
    .await
    .expect("读取事件计数")
}

async fn proof_listing_transition(
    pool: &PgPool,
    command_id: &str,
    version: i64,
    state: &str,
    reason: Option<&str>,
) {
    let reason_json = reason.map_or_else(|| "null".to_owned(), |reason| format!("\"{reason}\""));
    let reason_sql = reason.map_or_else(|| "NULL".to_owned(), |reason| format!("'{reason}'"));
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{command_id}', 2, 'worker', 'fake-worker', 'apply_mail_proof', '{ATTEMPT}',
                     decode(repeat('e1', 32), 'hex'), 'original_attempt', '{ATTEMPT}', NULL,
                     200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                        \"disposition\":\"take_proven\",\"command_id\":\"{command_id}\",
                        \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\",
                        \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",
                        \"listing_version\":{version},\"state\":\"{state}\"}}],
                        \"custody_changes\":[{{\"listing_id\":\"{LISTING}\",
                        \"listing_version\":{version},\"state\":\"{state}\",
                        \"held_reason\":{reason_json},\"position_id\":\"{POSITION}\",
                        \"position_state\":\"in_custody\"}}]}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', {version}, '{command_id}', 'worker',
                     '{event_kind}', 'deposit_accepted', '{state}', {reason_sql}, 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='{state}', reason_code={reason_sql}, state_version={version},
                 updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';",
            event_kind = if state == "available" { "available" } else { "hold_set" },
        ),
    )
    .await;
}

fn admin_hold_set_sql(
    command_id: &str,
    hold_id: &str,
    version: i64,
    body_reason: &str,
    event_reason: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command_id}', 2, 'admin', 'fake-admin', 'admin_delist',
                 '{LISTING}', decode(repeat('de', 32), 'hex'), 'listing', '{LISTING}',
                 '{LISTING}', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                    \"disposition\":\"hold_set\",\"command_id\":\"{command_id}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":{version},
                    \"state\":\"held_off_market\",\"held_reason\":\"{body_reason}\",
                    \"hold_id\":\"{hold_id}\"}}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', {version}, '{command_id}', 'admin', 'hold_set',
                 'deposit_accepted', 'held_off_market', '{event_reason}', 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state='held_off_market', reason_code='{event_reason}', state_version={version},
             updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
    )
}

fn release_hold_sql(
    command_id: &str,
    hold_id: &str,
    version: i64,
    body_reason: &str,
    event_reason: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{command_id}', 2, 'admin', 'fake-admin', 'release_hold',
                 '{LISTING}', decode(repeat('df', 32), 'hex'), 'listing', '{LISTING}',
                 '{LISTING}', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"release_hold\",
                    \"disposition\":\"hold_released\",\"command_id\":\"{command_id}\",
                    \"listing_id\":\"{LISTING}\",\"listing_version\":{version},
                    \"state\":\"held_off_market\",\"held_reason\":\"{body_reason}\",
                    \"hold_id\":\"{hold_id}\"}}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', {version}, '{command_id}', 'admin', 'hold_released',
                 'held_off_market', 'held_off_market', '{event_reason}', 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET reason_code='{event_reason}', state_version={version}, updated_at='{ACCEPTED}'
         WHERE id='{LISTING}';"
    )
}

#[tokio::test]
async fn postgres_consignment_v2_listing_issue_and_expire_reservation_transitions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_reservation()),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("awaiting_deposit".to_owned(), 3));
    assert_eq!(reservation_state(pool).await, ("active".to_owned(), None), "签发后的预约必须活动");

    let expire = expire_reservation_command()
        + &format!(
            "INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 4, '{command}', 'worker', 'reservation_retired',
                     'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state_version=4, updated_at='{ISSUED}' WHERE id='{LISTING}';
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='expired',
                 last_command_id='{command}'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, position_id=NULL, revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';",
            command = super::support::EXPIRE_COMMAND,
        );
    succeeds(pool, &expire).await;
    assert_eq!(listing_state(pool).await, ("awaiting_deposit".to_owned(), 4));
    assert_eq!(reservation_state(pool).await, ("retired".to_owned(), Some("expired".to_owned())));
    assert_eq!(event_count(pool, super::support::EXPIRE_COMMAND, "reservation_retired").await, 1);
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder, None, "到期退休必须释放实物占用");
}

#[tokio::test]
async fn postgres_consignment_v2_listing_envelope_and_quarantine_accept_transitions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    assert_eq!(listing_state(pool).await, ("deposit_accepted".to_owned(), 4));
    assert_eq!(event_count(pool, DECIDE, "deposit_accepted").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit() + &mail_session())).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{RELEASE_COMMAND}', 2, 'admin', 'fake-admin', 'release_quarantine',
                     '91000000-0000-0000-0000-000000000001', decode(repeat('e2', 32), 'hex'),
                     'quarantine', '91000000-0000-0000-0000-000000000001', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                        \"disposition\":\"quarantine_released\",\"command_id\":\"{RELEASE_COMMAND}\",
                        \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                        \"release_id\":\"{RELEASE}\",\"decision_id\":\"{RELEASE_DECISION}\",
                        \"decision_kind\":\"accept\",\"listing_changes\":[
                          {{\"listing_id\":\"{LISTING}\",\"listing_version\":3,
                            \"state\":\"deposit_accepted\"}}]}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{RELEASE_COMMAND}', 'admin', 'deposit_accepted',
                     'awaiting_deposit', 'deposit_accepted', NULL, 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='deposit_accepted', state_version=3, updated_at='{ISSUED}'
             WHERE id='{LISTING}';",
            RELEASE_COMMAND = super::support::RELEASE_COMMAND,
            RELEASE = super::support::RELEASE,
            RELEASE_DECISION = super::support::RELEASE_DECISION,
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("deposit_accepted".to_owned(), 3));
    assert_eq!(event_count(pool, super::support::RELEASE_COMMAND, "deposit_accepted").await, 1);
}

#[tokio::test]
async fn postgres_consignment_v2_listing_mail_proof_transitions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    assert_eq!(listing_state(pool).await, ("available".to_owned(), 5));
    assert_eq!(event_count(pool, PROOF_AVAILABLE, "available").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_HELD, 5, "held_off_market", Some("quota_blocked")).await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 5));
    assert_eq!(event_count(pool, PROOF_HELD, "hold_set").await, 1);
}

#[tokio::test]
async fn postgres_consignment_v2_listing_withdraw_transitions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_AVAILABLE}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('e3', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_AVAILABLE}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":6,\"state\":\"withdrawal_pending\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{WITHDRAW_AVAILABLE}', 'public_user',
                     'withdrawal_requested', 'available', 'withdrawal_pending', NULL, 101,
                     '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='withdrawal_pending', state_version=6, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("withdrawal_pending".to_owned(), 6));
    assert_eq!(event_count(pool, WITHDRAW_AVAILABLE, "withdrawal_requested").await, 1);

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
             VALUES ('{WITHDRAW_DEPOSIT}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('e4', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_DEPOSIT}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":5,\"state\":\"deposit_accepted\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{WITHDRAW_DEPOSIT}', 'public_user',
                     'withdrawal_requested', 'deposit_accepted', 'deposit_accepted', NULL, 101,
                     '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state_version=5, updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("deposit_accepted".to_owned(), 5));
    assert_eq!(event_count(pool, WITHDRAW_DEPOSIT, "withdrawal_requested").await, 1);

    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{ADMIN_HOLD_DEPOSIT}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('e5', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_DEPOSIT}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":6,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_DEPOSIT}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{ADMIN_HOLD_DEPOSIT}', 'admin', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=6,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_HELD}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('e6', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_HELD}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":7,\"state\":\"held_off_market\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 7, '{WITHDRAW_HELD}', 'public_user',
                     'withdrawal_requested', 'held_off_market', 'held_off_market',
                     'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state_version=7, updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 7));
    assert_eq!(event_count(pool, WITHDRAW_HELD, "withdrawal_requested").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_reservation()),
    )
    .await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_CLOSED}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('e7', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"closed\",\"command_id\":\"{WITHDRAW_CLOSED}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":4,
                        \"state\":\"closed\"}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 4, '{WITHDRAW_CLOSED}', 'public_user', 'closed',
                     'awaiting_deposit', 'closed', 'seller_cancelled', 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='seller_cancelled', state_version=4,
                 updated_at='{ISSUED}' WHERE id='{LISTING}';
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='cancelled',
                 last_command_id='{WITHDRAW_CLOSED}'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, position_id=NULL, revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 4));
    assert_eq!(reservation_state(pool).await, ("retired".to_owned(), Some("cancelled".to_owned())));
    assert_eq!(event_count(pool, WITHDRAW_CLOSED, "closed").await, 1);
}

#[tokio::test]
async fn postgres_consignment_v2_listing_admin_delist_and_release_hold_transitions() {
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
             VALUES ('{ADMIN_HOLD_DEPOSIT}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('e8', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_DEPOSIT}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_DEPOSIT}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{ADMIN_HOLD_DEPOSIT}', 'admin', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=5,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 5));
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{ADMIN_HOLD_AGAIN}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('e9', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_AGAIN}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":6,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_AGAIN}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{ADMIN_HOLD_AGAIN}', 'admin', 'hold_set',
                     'held_off_market', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state_version=6, updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 6));
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{RELEASE_HOLD}', 2, 'admin', 'fake-admin', 'release_hold',
                     '{LISTING}', decode(repeat('ea', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_hold\",
                        \"disposition\":\"hold_released\",\"command_id\":\"{RELEASE_HOLD}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":7,
                        \"state\":\"held_off_market\",\"held_reason\":\"review\",
                        \"hold_id\":\"{HOLD_AGAIN}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 7, '{RELEASE_HOLD}', 'admin', 'hold_released',
                     'held_off_market', 'held_off_market', 'review', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET reason_code='review', state_version=7, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 7));
    assert_eq!(event_count(pool, RELEASE_HOLD, "hold_released").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{ADMIN_HOLD_DEPOSIT}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('eb', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_DEPOSIT}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":6,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_DEPOSIT}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{ADMIN_HOLD_DEPOSIT}', 'admin', 'hold_set',
                     'available', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=6,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("held_off_market".to_owned(), 6));

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit())).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{ADMIN_CLOSED}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('ec', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"closed\",\"command_id\":\"{ADMIN_CLOSED}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":3,
                        \"state\":\"closed\"}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{ADMIN_CLOSED}', 'admin', 'closed',
                     'awaiting_deposit', 'closed', 'admin_delisted', 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='admin_delisted', state_version=3,
                 updated_at='{ISSUED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 3));
    assert_eq!(event_count(pool, ADMIN_CLOSED, "closed").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_reservation()),
    )
    .await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{ADMIN_CLOSED}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('ed', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"closed\",\"command_id\":\"{ADMIN_CLOSED}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":4,
                        \"state\":\"closed\"}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 4, '{ADMIN_CLOSED}', 'admin', 'closed',
                     'awaiting_deposit', 'closed', 'admin_delisted', 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='admin_delisted', state_version=4,
                 updated_at='{ISSUED}' WHERE id='{LISTING}';
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='{ISSUED}', reason='cancelled',
                 last_command_id='{ADMIN_CLOSED}'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, position_id=NULL, revision=revision+1,
                 changed_at='{ISSUED}', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
    )
    .await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 4));
    assert_eq!(reservation_state(pool).await, ("retired".to_owned(), Some("cancelled".to_owned())));
}

fn season_close_sql(version: i64, from_state: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_season_id, target_calendar_version,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{SEASON_CLOSE}', 2, 'worker', 'fake-worker', 'season_close', '{SEASON_ID}',
                 decode(repeat('ee', 32), 'hex'), 'season', '{SEASON_ID}',
                 decode(repeat('ef', 32), 'hex'), NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"season_close\",
                    \"disposition\":\"season_closed\",\"command_id\":\"{SEASON_CLOSE}\",
                    \"season_id\":\"{SEASON_ID}\",
                    \"target\":{{\"kind\":\"listing\",\"id\":\"{LISTING}\"}},
                    \"closed_listing_ids\":[\"{LISTING}\"]}}', '{ACCEPTED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{LISTING}', {version}, '{SEASON_CLOSE}', 'worker', 'season_closed',
                 '{from_state}', 'closed', 'season_closed', 101, '{ACCEPTED}');
         UPDATE public.consignment_listings
         SET state='closed', reason_code='season_closed', state_version={version},
             updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
    )
}

#[tokio::test]
async fn postgres_consignment_v2_listing_season_close_transitions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    succeeds(pool, &season_close_sql(6, "available")).await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 6));
    assert_eq!(event_count(pool, SEASON_CLOSE, "season_closed").await, 1);

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &season_close_sql(5, "deposit_accepted")).await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 5));

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
             VALUES ('{ADMIN_HOLD_DEPOSIT}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('f0', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_DEPOSIT}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_DEPOSIT}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{ADMIN_HOLD_DEPOSIT}', 'admin', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=5,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    succeeds(pool, &season_close_sql(6, "held_off_market")).await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 6));

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_AVAILABLE}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('f1', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_AVAILABLE}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":6,\"state\":\"withdrawal_pending\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{WITHDRAW_AVAILABLE}', 'public_user',
                     'withdrawal_requested', 'available', 'withdrawal_pending', NULL, 101,
                     '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='withdrawal_pending', state_version=6, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
    )
    .await;
    succeeds(pool, &season_close_sql(7, "withdrawal_pending")).await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 7));

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_resolving())).await;
    succeeds(pool, &season_close_sql(2, "resolving")).await;
    assert_eq!(listing_state(pool).await, ("closed".to_owned(), 2));
}

#[tokio::test]
async fn postgres_consignment_v2_listing_version_event_rejects_mismatched_commands() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit())).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V4}', 2, 'admin', 'fake-admin', 'release_quarantine',
                     '91000000-0000-0000-0000-000000000001', decode(repeat('f2', 32), 'hex'),
                     'quarantine', '91000000-0000-0000-0000-000000000001', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                        \"disposition\":\"quarantine_released\",\"command_id\":\"{OTHER_COMMAND_V4}\",
                        \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                        \"release_id\":\"{RELEASE}\",\"decision_id\":\"{RELEASE_DECISION}\",
                        \"decision_kind\":\"accept\",\"listing_changes\":[
                          {{\"listing_id\":\"{OTHER_LISTING}\",\"listing_version\":3,
                            \"state\":\"deposit_accepted\"}}]}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{OTHER_COMMAND_V4}', 'admin', 'deposit_accepted',
                     'awaiting_deposit', 'deposit_accepted', NULL, 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='deposit_accepted', state_version=3, updated_at='{ISSUED}'
             WHERE id='{LISTING}';",
            RELEASE = super::support::RELEASE,
            RELEASE_DECISION = super::support::RELEASE_DECISION,
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V5}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                     '{ATTEMPT}', decode(repeat('f3', 32), 'hex'), 'original_attempt',
                     '{ATTEMPT}', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                        \"disposition\":\"take_proven\",\"command_id\":\"{OTHER_COMMAND_V5}\",
                        \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\",
                        \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",
                        \"listing_version\":5,\"state\":\"available\"}}]}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{OTHER_COMMAND_V5}', 'worker', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'quota_blocked', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='quota_blocked', state_version=5,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V4}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('f4', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{OTHER_COMMAND_V4}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":5,\"state\":\"withdrawal_pending\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{OTHER_COMMAND_V4}', 'public_user',
                     'withdrawal_requested', 'deposit_accepted', 'deposit_accepted', NULL,
                     101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state_version=5, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit())).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V4}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('f5', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"closed\",\"command_id\":\"{OTHER_COMMAND_V4}\",
                        \"listing_id\":\"{OTHER_LISTING}\",\"listing_version\":3,
                        \"state\":\"closed\"}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{OTHER_COMMAND_V4}', 'public_user', 'closed',
                     'awaiting_deposit', 'closed', 'seller_cancelled', 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='seller_cancelled', state_version=3,
                 updated_at='{ISSUED}' WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_season_id, target_calendar_version,
                 listing_id, result_status, result_code, result_body, created_at)
             VALUES ('{OTHER_COMMAND_V5}', 2, 'worker', 'fake-worker', 'season_close',
                     '{SEASON_ID}', decode(repeat('f6', 32), 'hex'), 'season', '{SEASON_ID}',
                     decode(repeat('f7', 32), 'hex'), NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"season_close\",
                        \"disposition\":\"season_closed\",\"command_id\":\"{OTHER_COMMAND_V5}\",
                        \"season_id\":\"{SEASON_ID}\",
                        \"target\":{{\"kind\":\"listing\",\"id\":\"{OTHER_LISTING}\"}},
                        \"closed_listing_ids\":[\"{OTHER_LISTING}\"]}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{OTHER_COMMAND_V5}', 'worker', 'season_closed',
                     'available', 'closed', 'season_closed', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='closed', reason_code='season_closed', state_version=6,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_reservation()),
    )
    .await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V5}', 2, 'worker', 'fake-worker', 'expire_reservation',
                     '{OTHER_RESERVATION}', decode(repeat('f8', 32), 'hex'), 'reservation',
                     '{OTHER_RESERVATION}', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"expire_reservation\",
                        \"disposition\":\"reservation_retired\",
                        \"command_id\":\"{OTHER_COMMAND_V5}\",
                        \"reservation_id\":\"{OTHER_RESERVATION}\"}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 4, '{OTHER_COMMAND_V5}', 'worker', 'reservation_retired',
                     'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
             UPDATE public.consignment_listings
             SET state_version=4, updated_at='{ISSUED}' WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_v2_listing_event_edge_rejects_invalid_actor_or_reason() {
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
             VALUES ('{ADMIN_HOLD_DEPOSIT}', 2, 'admin', 'fake-admin', 'admin_delist',
                     '{LISTING}', decode(repeat('f9', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                        \"disposition\":\"hold_set\",\"command_id\":\"{ADMIN_HOLD_DEPOSIT}\",
                        \"listing_id\":\"{LISTING}\",\"listing_version\":5,
                        \"state\":\"held_off_market\",\"held_reason\":\"operations_hold\",
                        \"hold_id\":\"{HOLD_DEPOSIT}\"}}',
                     '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{ADMIN_HOLD_DEPOSIT}', 'admin', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='operations_hold', state_version=5,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
    )
    .await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_HELD}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('fa', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_HELD}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":6,\"state\":\"held_off_market\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{WITHDRAW_HELD}', 'admin', 'withdrawal_requested',
                     'held_off_market', 'held_off_market', 'operations_hold', 101, '{ACCEPTED}');"
        ),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    proof_listing_transition(pool, PROOF_AVAILABLE, 5, "available", None).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_season_id, target_calendar_version,
                 listing_id, result_status, result_code, result_body, created_at)
             VALUES ('{SEASON_CLOSE}', 2, 'worker', 'fake-worker', 'season_close',
                     '{SEASON_ID}', decode(repeat('fb', 32), 'hex'), 'season', '{SEASON_ID}',
                     decode(repeat('fc', 32), 'hex'), NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"season_close\",
                        \"disposition\":\"season_closed\",\"command_id\":\"{SEASON_CLOSE}\",
                        \"season_id\":\"{SEASON_ID}\",
                        \"target\":{{\"kind\":\"listing\",\"id\":\"{LISTING}\"}},
                        \"closed_listing_ids\":[\"{LISTING}\"]}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{SEASON_CLOSE}', 'worker', 'season_closed',
                     'available', 'closed', 'operations_hold', 101, '{ACCEPTED}');"
        ),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &(users_and_key() + &listing_awaiting_deposit())).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_COMMAND_V4}', 2, 'admin', 'fake-admin', 'release_quarantine',
                     '91000000-0000-0000-0000-000000000001', decode(repeat('fd', 32), 'hex'),
                     'quarantine', '91000000-0000-0000-0000-000000000001', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"release_quarantine\",
                        \"disposition\":\"quarantine_released\",
                        \"command_id\":\"{OTHER_COMMAND_V4}\",
                        \"quarantine_id\":\"91000000-0000-0000-0000-000000000001\",
                        \"release_id\":\"{RELEASE}\",\"decision_id\":\"{RELEASE_DECISION}\",
                        \"decision_kind\":\"accept\",\"listing_changes\":[
                          {{\"listing_id\":\"{LISTING}\",\"listing_version\":3,
                            \"state\":\"deposit_accepted\"}}]}}', '{ISSUED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 3, '{OTHER_COMMAND_V4}', 'public_user', 'deposit_accepted',
                     'awaiting_deposit', 'deposit_accepted', NULL, 101, '{ISSUED}');",
            RELEASE = super::support::RELEASE,
            RELEASE_DECISION = super::support::RELEASE_DECISION,
        ),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_v2_listing_reason_binding_rejects_rewrites() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(
        pool,
        &admin_hold_set_sql(
            ADMIN_HOLD_REWRITE,
            HOLD_REWRITE,
            5,
            "operations_hold",
            "operations_hold",
        ),
    )
    .await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{WITHDRAW_REWRITE}', 2, 'public_user', '{USER_1}', 'withdraw',
                     '{LISTING}', decode(repeat('e0', 32), 'hex'), 'listing', '{LISTING}',
                     '{LISTING}', 202, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"withdraw\",
                        \"disposition\":\"withdrawal_requested\",
                        \"command_id\":\"{WITHDRAW_REWRITE}\",\"listing_id\":\"{LISTING}\",
                        \"listing_version\":6,\"state\":\"held_off_market\",
                        \"intent_id\":\"{INTENT}\"}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 6, '{WITHDRAW_REWRITE}', 'public_user',
                     'withdrawal_requested', 'held_off_market', 'held_off_market', 'review',
                     101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET reason_code='review', state_version=6, updated_at='{ACCEPTED}'
             WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    fails(
        pool,
        &admin_hold_set_sql(
            ADMIN_HOLD_MISMATCH,
            HOLD_MISMATCH,
            5,
            "operations_hold",
            "quota_blocked",
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(
        pool,
        &admin_hold_set_sql(
            ADMIN_HOLD_REWRITE,
            HOLD_REWRITE,
            5,
            "operations_hold",
            "operations_hold",
        ),
    )
    .await;
    fails(
        pool,
        &release_hold_sql(RELEASE_NON_REVIEW, HOLD_REWRITE, 6, "review", "operations_hold"),
        "23514",
        Some("consignment_event_edge"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(
        pool,
        &admin_hold_set_sql(
            ADMIN_HOLD_MISMATCH,
            HOLD_MISMATCH,
            5,
            "operations_hold",
            "operations_hold",
        ),
    )
    .await;
    fails(
        pool,
        &release_hold_sql(RELEASE_MISMATCH, HOLD_MISMATCH, 6, "operations_hold", "review"),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    succeeds(pool, &attempt_start_proof_and_custody()).await;
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, target_kind, target_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{PROOF_BAD_REASON}', 2, 'worker', 'fake-worker', 'apply_mail_proof',
                     '{ATTEMPT}', decode(repeat('f0', 32), 'hex'), 'original_attempt',
                     '{ATTEMPT}', NULL, 200, 'ok',
                     '{{\"schema_version\":2,\"operation\":\"apply_mail_proof\",
                        \"disposition\":\"take_proven\",\"command_id\":\"{PROOF_BAD_REASON}\",
                        \"decision_id\":\"{DECISION}\",\"proof_id\":\"{PROOF}\",
                        \"listing_changes\":[{{\"listing_id\":\"{LISTING}\",
                        \"listing_version\":5,\"state\":\"held_off_market\"}}],
                        \"custody_changes\":[{{\"listing_id\":\"{LISTING}\",
                        \"listing_version\":5,\"state\":\"held_off_market\",
                        \"held_reason\":\"unpaid_recovered\",\"position_id\":\"{POSITION}\",
                        \"position_state\":\"in_custody\"}}]}}', '{ACCEPTED}');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{LISTING}', 5, '{PROOF_BAD_REASON}', 'worker', 'hold_set',
                     'deposit_accepted', 'held_off_market', 'unpaid_recovered', 101, '{ACCEPTED}');
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='unpaid_recovered', state_version=5,
                 updated_at='{ACCEPTED}' WHERE id='{LISTING}';"
        ),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;

    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim_reservation()),
    )
    .await;
    fails(
        pool,
        &(expire_reservation_command()
            + &format!(
                "INSERT INTO public.consignment_events
                    (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                     to_state, reason_code, price, occurred_at)
                 VALUES ('{LISTING}', 4, '{command}', 'worker', 'reservation_retired',
                         'awaiting_deposit', 'awaiting_deposit', NULL, 101, '{ISSUED}');
                 UPDATE public.consignment_listings
                 SET reason_code='seller_ineligible', state_version=4, updated_at='{ISSUED}'
                 WHERE id='{LISTING}';",
                command = super::support::EXPIRE_COMMAND,
            )),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
}
