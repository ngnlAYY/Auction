//! 4B 实物强占用：无空档转 position、直接 lot 插入守卫与释放后重列。

use auction_application::consignment::ReservationStore;
use auction_domain::consignment::deposit::MatchedDepositItem;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingVersion, OriginalMailKey};
use auction_domain::{SettlementWorld, UserId};
use uuid::Uuid;

use super::super::lot_guard::{insert_equipment, seed_auction};
use super::super::support::{
    ACCEPTED, DECIDE, DECISION, ENVELOPE, ISSUED, LISTING, OBSERVATION, RESERVATION, USER_1,
};
use super::super::support::{
    listing_awaiting_deposit, mail_session, observation_and_asset, reservation_issued, succeeds,
    users_and_key,
};
use super::helpers::{client_command, expire_command, repository, worker_command};
use crate::support::TestDatabase;

fn reservation_claim() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    )
}

fn listing_awaiting_future() -> String {
    listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
}

fn reservation_only() -> String {
    users_and_key()
        + &listing_awaiting_future()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &reservation_claim()
}

fn accepted_without_items() -> String {
    reservation_only()
}

fn item() -> MatchedDepositItem {
    MatchedDepositItem {
        ordinal: 1,
        listing_id: Uuid::parse_str(LISTING).expect("固定挂单"),
        listing_version: ListingVersion::new(3).expect("版本"),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家")),
        reservation_id: Uuid::parse_str(RESERVATION).expect("固定预约"),
        reservation_generation: 1,
        equipment: ListingEquipmentIdentity::new("main", "42").expect("固定装备"),
        credential_ref: Some(Uuid::parse_str(ENVELOPE).expect("固定凭证")),
    }
}

#[tokio::test]
async fn postgres_reservation_transfers_claim_to_position_without_gap() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &accepted_without_items()).await;
    let repository = repository(&database).await;
    let accepted = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键"),
            vec![item()],
            auction_time::Timestamp::parse_from_rfc3339(ACCEPTED).expect("接受时刻"),
        )
        .await
        .expect("整封接受");
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].listing_version, 4);
    let position = accepted[0].position_id;
    let claim: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT reservation_id::text, position_id::text, revision
         FROM public.consignment_equipment_claims WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(claim, (None, Some(position.to_string()), 2));
    let reservation: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_deposit_reservations WHERE id=$1")
            .bind(Uuid::parse_str(RESERVATION).expect("预约"))
            .fetch_one(pool)
            .await
            .expect("读取预约");
    assert_eq!(reservation, "consumed");
    let stored_position: (String, String) = sqlx::query_as(
        "SELECT state, version::text FROM public.consignment_custody_positions WHERE id=$1",
    )
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("读取 position");
    assert_eq!(stored_position, ("awaiting_take".to_owned(), "1".to_owned()));
    let items: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id=$1",
    )
    .bind(Uuid::parse_str(DECISION).expect("决定"))
    .fetch_one(pool)
    .await
    .expect("读取 item");
    assert_eq!(items, 1);
}

#[tokio::test]
async fn postgres_raw_lot_insert_cannot_bypass_stale_equipment_snapshot() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &reservation_only()).await;
    let auction = seed_auction(pool, 21).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000021", "42").await;
    let lot = "62000000-0000-0000-0000-000000000021";
    super::super::support::fails(
        pool,
        &format!(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000021', 21,
                     'unsold', 100, 'Weapon', 21);"
        ),
        "23514",
        Some("c4_claims_equipment_busy"),
    )
    .await;
    let repository = repository(&database).await;
    let expires_at: auction_time::Timestamp = sqlx::query_scalar(
        "SELECT expires_at FROM public.consignment_deposit_reservations WHERE id=$1",
    )
    .bind(Uuid::parse_str(RESERVATION).expect("预约"))
    .fetch_one(pool)
    .await
    .expect("读取预约截止");
    repository
        .expire_reservation(expire_command(
            worker_command(21),
            Uuid::parse_str(RESERVATION).expect("预约"),
            1,
            expires_at,
        ))
        .await
        .expect("合法退休");
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000021', 21,
                     'unsold', 100, 'Weapon', 21);"
        ),
    )
    .await;
}

#[tokio::test]
async fn postgres_reservation_races_mail_admin_and_recovery_relist() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &reservation_only()).await;
    let auction = seed_auction(pool, 22).await;
    insert_equipment(pool, "61000000-0000-0000-0000-000000000022", "42").await;
    let lot = "62000000-0000-0000-0000-000000000022";
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let lot_pool = pool.clone();
    let lot_barrier = barrier.clone();
    let lot_task = tokio::spawn(async move {
        let mut transaction =
            lot_pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await.expect("开启 lot 事务");
        lot_barrier.wait().await;
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price,
                 category_key, category_number)
             VALUES ('{lot}', '{auction}', '61000000-0000-0000-0000-000000000022', 22,
                     'unsold', 100, 'Weapon', 22);"
        )))
        .execute(&mut *transaction)
        .await;
        if result.is_err() {
            transaction.rollback().await.expect("回滚 lot");
            return false;
        }
        transaction.commit().await.is_ok()
    });
    let repository = repository(&database).await;
    let expire_barrier = barrier.clone();
    let expire_task = tokio::spawn(async move {
        expire_barrier.wait().await;
        let expires_at: auction_time::Timestamp = sqlx::query_scalar(
            "SELECT expires_at FROM public.consignment_deposit_reservations WHERE id=$1",
        )
        .bind(Uuid::parse_str(RESERVATION).expect("预约"))
        .fetch_one(repository.store().postgres_pool())
        .await
        .expect("读取预约截止");
        repository
            .expire_reservation(expire_command(
                worker_command(22),
                Uuid::parse_str(RESERVATION).expect("预约"),
                1,
                expires_at,
            ))
            .await
            .is_ok()
    });
    let lot_ok = lot_task.await.expect("lot 任务");
    let expire_ok = expire_task.await.expect("退休任务");
    assert!(lot_ok ^ expire_ok, "同一实物竞争只能一方提交");
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.lots WHERE id=$1)")
        .bind(Uuid::parse_str(lot).expect("lot"))
        .fetch_one(pool)
        .await
        .expect("读取 lot");
    assert_eq!(exists, lot_ok);
}

#[allow(dead_code)]
fn _accepted(_: &str) -> &str {
    ACCEPTED
}

#[allow(dead_code)]
fn _observation(_: Uuid) -> Uuid {
    Uuid::parse_str(OBSERVATION).expect("观察")
}

#[allow(dead_code)]
fn _client(_: u128) -> auction_domain::CommandId {
    client_command(1)
}
