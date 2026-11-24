//! 4B 预约签发前的同件强占用反例：另一挂单的预约与已存在的 position。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, IssueReservationCommand, ReservationStore,
};
use auction_domain::UserId;
use auction_domain::consignment::ListingVersion;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{self, ISSUED, LISTING, RESERVATION, SESSION, USER_1, USER_2};
use super::super::support::{
    listing_awaiting_deposit, mail_session, observation_and_asset, reservation_issued, succeeds,
    users_and_key,
};
use super::helpers::{client_command, hash, repository};
use crate::support::TestDatabase;

fn listing_awaiting_future() -> String {
    listing_awaiting_deposit().replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
}

fn reservation_claim() -> String {
    format!(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '42', 1, '{RESERVATION}', NULL, '{ISSUED}', '{RESERVATION}');"
    )
}

fn reservation_only() -> String {
    users_and_key()
        + &listing_awaiting_future()
        + &mail_session()
        + &reservation_issued()
        + &observation_and_asset()
        + &reservation_claim()
}

async fn seed_readiness(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ('main', $1, 1, decode(repeat('35', 32), 'hex'), 'enforced',
                 ARRAY['FakeSellerOne', 'FakeSellerTwo'], ARRAY[]::text[], true, true, true,
                 clock_timestamp(), clock_timestamp() + interval '1 day', 1)",
    )
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .execute(pool)
    .await
    .expect("写入收件就绪");
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1",
    )
    .execute(pool)
    .await
    .expect("打开解析闸门");
}

async fn seed_second_awaiting(pool: &PgPool, listing_id: Uuid) {
    let snapshot = Uuid::new_v4();
    let register = Uuid::new_v4();
    let publish = Uuid::new_v5(&Uuid::NAMESPACE_URL, listing_id.as_bytes());
    let sql = listing_awaiting_deposit()
        .replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
        .replace(LISTING, &listing_id.to_string())
        .replace("30000000-0000-0000-0000-000000000001", &snapshot.to_string())
        .replace("40000000-0000-4000-8000-000000000001", &register.to_string())
        .replace("41000000-0000-5000-8000-000000000001", &publish.to_string())
        .replace(USER_1, USER_2);
    succeeds(pool, &sql).await;
}

fn seller_two_command(listing_id: Uuid, seed: u128) -> IssueReservationCommand {
    IssueReservationCommand {
        command_id: client_command(seed),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_2).expect("第二卖家")),
        listing_id,
        expected_version: ListingVersion::new(2).expect("版本"),
        request_hash: hash(0xe1),
        request_id: None,
    }
}

#[tokio::test]
async fn postgres_issue_reservation_rejects_another_listings_active_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    succeeds(pool, &reservation_only()).await;
    seed_readiness(pool).await;
    let maintenance = database.maintenance_store().await.expect("维护账号");
    super::super::support::execute(
        maintenance.postgres_pool(),
        &format!(
            "ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
             UPDATE public.consignment_listings
             SET state='held_off_market', reason_code='review', state_version=4,
                 updated_at=transaction_timestamp()
             WHERE id='{LISTING}';
             ALTER TABLE public.consignment_listings ENABLE TRIGGER USER;"
        ),
    )
    .await
    .expect("把原挂单移出已发布集合");
    let second = Uuid::new_v4();
    seed_second_awaiting(pool, second).await;
    let repository = repository(&database).await;
    let error = repository
        .issue_reservation(seller_two_command(second, 300))
        .await
        .expect_err("另一挂单的预约必须阻止签发");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::EquipmentBusy)));
}

#[tokio::test]
async fn postgres_issue_reservation_rejects_an_existing_position_claim() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    seed_readiness(pool).await;
    let second = Uuid::new_v4();
    seed_second_awaiting(pool, second).await;
    let repository = repository(&database).await;
    let error = repository
        .issue_reservation(seller_two_command(second, 301))
        .await
        .expect_err("position 强占用必须阻止签发");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::EquipmentBusy)));
}
