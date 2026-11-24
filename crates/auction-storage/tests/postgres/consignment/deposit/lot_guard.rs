//! lot 实物准入与寄售强占用共用互斥行：强占用拒绝、空闲推进、并发恰一成功。

use std::sync::Arc;

use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use tokio::sync::Barrier;

use super::support::{
    ISSUED, REISSUE_COMMAND, RESERVATION, START, USER_1, fails, reissue_reservation_command,
    seed_accepted, succeeds,
};
use crate::support::{TestDatabase, fixtures};

pub(super) async fn seed_auction(pool: &PgPool, sequence: i64) -> String {
    let deadline = Timestamp::now() + Duration::hours(6);
    let auction_id = format!("60000000-0000-0000-0000-{:012}", sequence);
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1::uuid, 'main', $2, 'Fake lot guard', 'live', $3, $3, $3, $3, $4)",
    )
    .bind(&auction_id)
    .bind(sequence)
    .bind(deadline)
    .bind(fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(pool)
    .await
    .expect("写入测试场次");
    auction_id
}

pub(super) async fn insert_equipment(pool: &PgPool, id: &str, eid: &str) {
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1::uuid, 'main', $2, 'Fake lot guard', 'Weapon', '{}'::jsonb, $3)",
    )
    .bind(id)
    .bind(eid)
    .bind(START.parse::<Timestamp>().expect("测试时刻"))
    .execute(pool)
    .await
    .expect("写入测试装备");
}

fn lot_sql(lot: &str, auction: &str, equipment: &str, number: i32) -> String {
    format!(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price,
             category_key, category_number)
         VALUES ('{lot}', '{auction}', '{equipment}', {number}, 'unsold', 100,
                 'Weapon', {number});"
    )
}

#[tokio::test]
async fn postgres_consignment_lot_guard_rejects_a_strong_claim() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    seed_accepted(pool).await;
    let auction = seed_auction(pool, 1).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000001", "42").await;
    fails(
        pool,
        &lot_sql(
            "62000000-0000-0000-0000-000000000001",
            &auction,
            "61000000-0000-0000-0000-000000000001",
            1,
        ),
        "23514",
        Some("c4_claims_equipment_busy"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_lot_guard_advances_an_idle_revision() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let auction = seed_auction(pool, 2).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000002", "43").await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000003", "043").await;
    succeeds(
        pool,
        &lot_sql(
            "62000000-0000-0000-0000-000000000002",
            &auction,
            "61000000-0000-0000-0000-000000000002",
            2,
        ),
    )
    .await;
    succeeds(
        pool,
        &lot_sql(
            "62000000-0000-0000-0000-000000000003",
            &auction,
            "61000000-0000-0000-0000-000000000003",
            3,
        ),
    )
    .await;
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='43'",
    )
    .fetch_one(pool)
    .await
    .expect("读取空闲占用版本");
    assert_eq!(revision, 2);
}

#[tokio::test]
async fn postgres_consignment_lot_guard_leaves_non_consignment_equipment_unchanged() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let auction = seed_auction(pool, 3).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000004", "legacy-eid").await;
    succeeds(
        pool,
        &lot_sql(
            "62000000-0000-0000-0000-000000000004",
            &auction,
            "61000000-0000-0000-0000-000000000004",
            4,
        ),
    )
    .await;
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='legacy-eid'",
    )
    .fetch_one(pool)
    .await
    .expect("读取非寄售占用");
    assert_eq!(claims, 0);
}

#[tokio::test]
async fn postgres_consignment_lot_guard_concurrent_claim_and_lot_have_one_winner() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    succeeds(pool, &super::support::users_and_key()).await;
    succeeds(pool, &super::support::listing_awaiting_deposit()).await;
    succeeds(pool, &super::support::mail_session()).await;
    succeeds(pool, &super::support::reservation_issued()).await;
    let auction = seed_auction(pool, 4).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000005", "44").await;
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.consignment_equipment_claims
                (world_id, canonical_eid, revision, reservation_id, position_id,
                 changed_at, last_fact_id)
             VALUES ('main', '44', 1, NULL, NULL, '{START}', '{USER_1}');"
        ),
    )
    .await;

    let barrier = Arc::new(Barrier::new(2));
    let lot_pool = pool.clone();
    let claim_pool = pool.clone();
    let lot_barrier = barrier.clone();
    let claim_barrier = barrier.clone();
    let lot_auction = auction.clone();
    let lot = tokio::spawn(async move {
        let mut transaction =
            lot_pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.expect("开启 lot 事务");
        lot_barrier.wait().await;
        let inserted = sqlx::query(sqlx::AssertSqlSafe(lot_sql(
            "62000000-0000-0000-0000-000000000005",
            &lot_auction,
            "61000000-0000-0000-0000-000000000005",
            5,
        )))
        .execute(&mut *transaction)
        .await;
        if inserted.is_err() {
            transaction.rollback().await.expect("回滚 lot");
            return false;
        }
        transaction.commit().await.is_ok()
    });
    let claim = tokio::spawn(async move {
        let mut transaction = claim_pool
            .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
            .await
            .expect("开启占用事务");
        claim_barrier.wait().await;
        let replacement = "82000000-0000-0000-0000-000000000002";
        let command = reissue_reservation_command(replacement);
        let prepared_command =
            sqlx::query(sqlx::AssertSqlSafe(command)).execute(&mut *transaction).await;
        if prepared_command.is_err() {
            transaction.rollback().await.expect("回滚换发命令");
            return false;
        }
        let prepared = sqlx::query(
            "UPDATE public.consignment_deposit_reservations
             SET state='retired', retired_at=$1, reason='reissued', last_command_id=$3::uuid
             WHERE id=$2::uuid",
        )
        .bind(ISSUED.parse::<Timestamp>().expect("测试时刻"))
        .bind(RESERVATION)
        .bind(REISSUE_COMMAND)
        .execute(&mut *transaction)
        .await;
        if prepared.is_err() {
            transaction.rollback().await.expect("回滚旧预约");
            return false;
        }
        let inserted = sqlx::query(
            "INSERT INTO public.consignment_deposit_reservations
                (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                 canonical_eid, season_window_id, generation, service_epoch, session_id,
                 token_digest, issued_at, expires_at, state, last_command_id, retired_at, reason)
             SELECT $1::uuid, listing_id, seller_user_id, verified_sender, bot_uid, world_id,
                    canonical_eid, season_window_id, 2, service_epoch, session_id,
                    decode(repeat('56', 32), 'hex'), issued_at, expires_at, 'active',
                    $3::uuid, NULL, NULL
             FROM public.consignment_deposit_reservations WHERE id=$2::uuid",
        )
        .bind(replacement)
        .bind(RESERVATION)
        .bind(REISSUE_COMMAND)
        .execute(&mut *transaction)
        .await;
        if inserted.is_err() {
            transaction.rollback().await.expect("回滚新预约");
            return false;
        }
        let updated = sqlx::query(
            "UPDATE public.consignment_equipment_claims
             SET revision=revision+1, reservation_id=$1, changed_at=$2, last_fact_id=$1
             WHERE world_id='main' AND canonical_eid='44'",
        )
        .bind(replacement)
        .bind(START.parse::<Timestamp>().expect("测试时刻"))
        .execute(&mut *transaction)
        .await;
        if updated.is_err() {
            transaction.rollback().await.expect("回滚占用");
            return false;
        }
        transaction.commit().await.is_ok()
    });
    let lot_ok = lot.await.expect("lot 任务");
    let claim_ok = claim.await.expect("占用任务");
    assert!(lot_ok ^ claim_ok, "恰一个写入应成功：lot={lot_ok}, claim={claim_ok}");
}
