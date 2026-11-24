//! 实物占用持有人变化必须由本事务写入的真实预约、item/position 或代管事件支撑。

use sqlx::PgPool;

use super::lot_guard::{insert_equipment, seed_auction};
use super::support::{
    ACCEPTED, EXPIRE_COMMAND, ISSUE, ISSUED, LISTING, POSITION, PUBLISH, RESERVATION, SESSION,
    START, USER_1, expire_reservation_command, fails, listing_awaiting_deposit, mail_session,
    reservation_issued, seed_accepted, succeeds, users_and_key,
};
use crate::support::TestDatabase;

async fn lot_exists(pool: &PgPool, lot: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM public.lots WHERE id=$1::uuid)")
        .bind(lot)
        .fetch_one(pool)
        .await
        .expect("读取 lot")
}

async fn row_count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM public.{table}"
    )))
    .fetch_one(pool)
    .await
    .expect("读取表计数")
}

#[tokio::test]
async fn postgres_consignment_claim_direct_release_then_lot_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    let auction = seed_auction(pool, 10).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000010", "42").await;
    let lot = "62000000-0000-0000-0000-000000000010";
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, position_id=NULL, revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{POSITION}'
             WHERE world_id='main' AND canonical_eid='42';
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000010', 10,
                     'unsold', 100, 'Weapon', 10);"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert!(!lot_exists(pool, lot).await, "直接释放后 lot 不应提交");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT position_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(POSITION));
}

#[tokio::test]
async fn postgres_consignment_claim_legal_reservation_release_then_lot_succeeds() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let claim = format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    );
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim),
    )
    .await;
    let auction = seed_auction(pool, 11).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000011", "42").await;
    let lot = "62000000-0000-0000-0000-000000000011";
    succeeds(
        pool,
        &format!(
            "{}UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='expired',
                 last_command_id='{EXPIRE_COMMAND}'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z',
                 last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000011', 11,
                     'unsold', 100, 'Weapon', 11);",
            expire_reservation_command()
        ),
    )
    .await;
    assert!(lot_exists(pool, lot).await, "带真实退休事实的释放后 lot 应提交");
    let claim: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT reservation_id::text, position_id::text, revision
         FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(claim, (None, None, 3));
}

#[tokio::test]
async fn postgres_consignment_claim_bare_reservation_retirement_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let claim = format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    );
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &claim),
    )
    .await;
    let commands_before = row_count(pool, "consignment_commands").await;
    let auction = seed_auction(pool, 12).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000012", "42").await;
    let lot = "62000000-0000-0000-0000-000000000012";
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='cancelled'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, position_id=NULL, revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000012', 12,
                     'unsold', 100, 'Weapon', 12);"
        ),
        "23514",
        Some("c4_res_transition"),
    )
    .await;
    assert_eq!(row_count(pool, "consignment_commands").await, commands_before);
    assert!(!lot_exists(pool, lot).await, "裸退休事务不得留下 lot");
    let claim: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT reservation_id::text, position_id::text, revision
         FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(claim, (Some(RESERVATION.to_owned()), None, 1));
    let reservation: (String, String) = sqlx::query_as(
        "SELECT state, last_command_id::text
         FROM public.consignment_deposit_reservations WHERE id=$1::uuid",
    )
    .bind(RESERVATION)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(reservation.0, "active");
    assert_ne!(reservation.1, EXPIRE_COMMAND);
}

#[tokio::test]
async fn postgres_consignment_claim_reservation_retirement_rejects_a_stale_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key() + &listing_awaiting_deposit() + &mail_session() + &reservation_issued()),
    )
    .await;
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='cancelled',
                 last_command_id='{PUBLISH}'
             WHERE id='{RESERVATION}';"
        ),
        "23514",
        Some("c4_res_transition_fact"),
    )
    .await;
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
async fn postgres_consignment_claim_release_rechecks_the_retirement_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &format!(
                "INSERT INTO public.consignment_equipment_claims
                    (world_id, canonical_eid, revision, reservation_id, position_id,
                     changed_at, last_fact_id)
                 VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}',
                         '{RESERVATION}');"
            )),
    )
    .await;
    let auction = seed_auction(pool, 13).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000013", "42").await;
    let lot = "62000000-0000-0000-0000-000000000013";
    fails(
        pool,
        &format!(
            "ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition;
             ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition_fact;
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='cancelled',
                 last_command_id='{PUBLISH}'
             WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id=NULL, revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';
             INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000013', 13,
                     'unsold', 100, 'Weapon', 13);"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    assert!(!lot_exists(pool, lot).await, "缺少命令的释放不得留下 lot");
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
    let enabled: String = sqlx::query_scalar(
        "SELECT tgenabled::text FROM pg_trigger
         WHERE tgname='c4_res_transition' AND NOT tgisinternal",
    )
    .fetch_one(pool)
    .await
    .expect("读取触发器状态");
    assert_eq!(enabled, "O", "失败事务应恢复预约触发器");
}

#[tokio::test]
async fn postgres_consignment_claim_swap_rechecks_the_old_retirement_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &format!(
                "INSERT INTO public.consignment_equipment_claims
                    (world_id, canonical_eid, revision, reservation_id, position_id,
                     changed_at, last_fact_id)
                 VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}',
                         '{RESERVATION}');"
            )),
    )
    .await;
    let second = "82000000-0000-0000-0000-000000000003";
    fails(
        pool,
        &format!(
            "ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition;
             ALTER TABLE public.consignment_deposit_reservations
                 DISABLE TRIGGER c4_res_transition_fact;
             UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at='2026-01-01T04:00:00Z', reason='reissued',
                 last_command_id='{PUBLISH}'
             WHERE id='{RESERVATION}';
             INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             SELECT '{second}', listing_id, seller_user_id, verified_sender, bot_uid,
                    world_id, canonical_eid, season_window_id, 2, service_epoch, session_id,
                    decode(repeat('64', 32), 'hex'), issued_at, expires_at, 'active',
                    '{ISSUE}', NULL, NULL
             FROM public.consignment_deposit_reservations WHERE id='{RESERVATION}';
             UPDATE public.consignment_equipment_claims
             SET reservation_id='{second}', revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{second}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
    let second_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_deposit_reservations WHERE id=$1::uuid)",
    )
    .bind(second)
    .fetch_one(pool)
    .await
    .expect("读取第二预约");
    assert!(!second_exists, "失败事务不得留下第二预约");
}

#[tokio::test]
async fn postgres_consignment_claim_direct_reservation_to_reservation_swap_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &format!(
                "INSERT INTO public.consignment_equipment_claims
                    (world_id, canonical_eid, revision, reservation_id, position_id,
                     changed_at, last_fact_id)
                 VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}',
                         '{RESERVATION}');"
            )),
    )
    .await;
    let second = "82000000-0000-0000-0000-000000000002";
    fails(
        pool,
        &format!(
            "ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
             ALTER TABLE public.consignment_deposit_reservations DISABLE TRIGGER USER;
             INSERT INTO public.consignment_listings
             SELECT (jsonb_populate_record(NULL::public.consignment_listings,
                     to_jsonb(listing) || '{{\"id\":\"20000000-0000-0000-0000-000000000002\",\"seller_user_id\":\"10000000-0000-0000-0000-000000000002\",\"state\":\"resolving\",\"state_version\":4,\"current_snapshot_id\":null,\"reason_code\":null}}'::jsonb)).*
             FROM public.consignment_listings AS listing
             WHERE listing.id='{LISTING}';
             INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             VALUES ('{second}', '20000000-0000-0000-0000-000000000002',
                     '10000000-0000-0000-0000-000000000002', 'FakeSellerTwo', '12345',
                     'main', '42', NULL, 1, 1,
                     '{SESSION}', decode(repeat('5f', 32), 'hex'),
                     '{ISSUED}', '2026-01-01T12:00:00Z', 'active',
                     '{ISSUE}', NULL, NULL);
             UPDATE public.consignment_equipment_claims
             SET reservation_id='{second}', revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{second}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(RESERVATION));
    let second_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_deposit_reservations WHERE id=$1::uuid)",
    )
    .bind(second)
    .fetch_one(pool)
    .await
    .expect("读取第二预约");
    assert!(!second_exists, "失败事务不得留下第二预约");
}

#[tokio::test]
async fn postgres_consignment_claim_direct_holder_swap_is_rejected() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_equipment_claims
             SET reservation_id='{RESERVATION}', position_id=NULL, revision=revision+1,
                 changed_at='2026-01-01T04:00:00Z', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT position_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder.as_deref(), Some(POSITION));
}

#[tokio::test]
async fn postgres_consignment_claim_acquisition_requires_same_transaction_fact() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_deposit()
            + &mail_session()
            + &reservation_issued()
            + &format!(
                "INSERT INTO public.consignment_equipment_claims
                    (world_id, canonical_eid, revision, reservation_id, position_id,
                     changed_at, last_fact_id)
                 VALUES ('main', '42', 1, NULL, NULL, '{START}', '{USER_1}');"
            )),
    )
    .await;
    fails(
        pool,
        &format!(
            "UPDATE public.consignment_equipment_claims
             SET reservation_id='{RESERVATION}', revision=revision+1,
                 changed_at='{ACCEPTED}', last_fact_id='{RESERVATION}'
             WHERE world_id='main' AND canonical_eid='42';"
        ),
        "23514",
        Some("c4_claim_transition_fact_ck"),
    )
    .await;
    let holder: Option<String> = sqlx::query_scalar(
        "SELECT reservation_id::text FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder, None);
}
