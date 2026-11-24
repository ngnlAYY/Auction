//! 4B 整封接受原语：真实 awaiting_deposit 起始、挂单版本、回执 L、截止等号与末件回滚。

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection};
use auction_domain::consignment::deposit::MatchedDepositItem;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingVersion, OriginalMailKey};
use auction_domain::{SettlementWorld, UserId};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{
    DECIDE, DECISION, ENVELOPE, ISSUED, LISTING, RESERVATION, SESSION, USER_1,
};
use super::super::support::{
    listing_awaiting_deposit, mail_session, observation_and_asset, reservation_issued, succeeds,
    users_and_key,
};
use super::helpers::repository;
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

async fn seed_first_item(pool: &PgPool) {
    succeeds(
        pool,
        &(users_and_key()
            + &listing_awaiting_future()
            + &mail_session()
            + &reservation_issued()
            + &observation_and_asset()
            + &reservation_claim()),
    )
    .await;
}

struct SecondItem {
    listing_id: Uuid,
    reservation_id: Uuid,
}

async fn seed_second_item(pool: &PgPool) -> SecondItem {
    let listing_id = Uuid::new_v4();
    let snapshot_id = Uuid::new_v4();
    let register_id = Uuid::new_v4();
    let publish_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"accept-second-publish");
    let listing = listing_awaiting_deposit()
        .replace("2026-01-02T00:00:00Z", "2099-01-01T00:00:00Z")
        .replace(LISTING, &listing_id.to_string())
        .replace("30000000-0000-0000-0000-000000000001", &snapshot_id.to_string())
        .replace("40000000-0000-4000-8000-000000000001", &register_id.to_string())
        .replace("41000000-0000-5000-8000-000000000001", &publish_id.to_string())
        .replace("'42'", "'43'");
    succeeds(pool, &listing).await;
    let reservation_id = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    let observation_id = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开启第二件夹具事务");
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1, $2, $3, 'FakeSellerOne', '12345', 'main', '43', NULL, 1, 1,
                 $4, decode(repeat('71', 32), 'hex'), $5,
                 timestamptz '2026-01-01T02:30:00Z', 'active', $6, NULL, NULL)",
    )
    .bind(reservation_id)
    .bind(listing_id)
    .bind(Uuid::parse_str(USER_1).expect("卖家"))
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .bind(auction_time::Timestamp::parse_from_rfc3339(ISSUED).expect("签发时刻"))
    .bind(register_id)
    .execute(&mut *transaction)
    .await
    .expect("写入第二件预约");
    sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('main', '43', 1, $1, NULL, $2, $1)",
    )
    .bind(reservation_id)
    .bind(auction_time::Timestamp::parse_from_rfc3339(ISSUED).expect("签发时刻"))
    .execute(&mut *transaction)
    .await
    .expect("写入第二件强占用");
    sqlx::query(
        "INSERT INTO public.mail_envelope_observations
            (id, bot_uid, world_id, external_mid, remote_observation_id, session_id,
             mailbox_key, quarantine_id, quarantine_version, observed_at, parser_version,
             evidence_ref, subject_digest, body_digest, manifest_digest, sender, recipient,
             returnable, takeable, credits, cod, raw_count, manifest, derived_facts,
             season_window_id)
         VALUES ($1, '12345', 'main', '555', 'obs-2', $2, 'fake-mailbox', NULL, NULL,
                 timestamptz '2026-01-01T05:00:00Z', 'parser-v1', NULL,
                 decode(repeat('72', 32), 'hex'), decode(repeat('73', 32), 'hex'),
                 decode(repeat('74', 32), 'hex'), 'FakeSellerOne', 'FakeBot',
                 true, true, 0, 0, 2,
                 '[{\"ordinal\":1,\"kind\":\"equipment\",\"name\":\"Fake Gear 42\",
                    \"canonical_eid\":\"42\"},
                   {\"ordinal\":2,\"kind\":\"equipment\",\"name\":\"Fake Gear 43\",
                    \"canonical_eid\":\"43\"}]'::jsonb,
                 '{\"subject_is_consign\":true,\"sender_identity\":\"verified\",
                   \"list_detail_consistent\":true,\"duplicate_raw_eid\":false,
                   \"canonical_alias_ambiguous\":false,\"unknown_attachment\":false,
                   \"system_rts\":false,\"legacy_auction_owner\":null,
                   \"received_minute_raw\":\"2026-01-01T05:00\",
                   \"items\":[{\"ordinal\":1,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true},
                              {\"ordinal\":2,\"world\":\"main\",\"tradeable\":true,
                               \"history_ok\":true}]}'::jsonb,
                 NULL)",
    )
    .bind(observation_id)
    .bind(Uuid::parse_str(SESSION).expect("会话"))
    .execute(&mut *transaction)
    .await
    .expect("写入两件观察");
    sqlx::query(
        "INSERT INTO public.consignment_custody_assets
            (equipment_id, world_id, canonical_eid, season_id, first_observation_id,
             auction_equipment_id)
         VALUES ($1, 'main', '43', NULL, $2, NULL)",
    )
    .bind(asset_id)
    .bind(observation_id)
    .execute(&mut *transaction)
    .await
    .expect("写入第二件资产");
    transaction.commit().await.expect("提交第二件夹具");
    SecondItem { listing_id, reservation_id }
}

fn item(ordinal: u32, listing_id: Uuid, eid: &str, reservation_id: Uuid) -> MatchedDepositItem {
    MatchedDepositItem {
        ordinal,
        listing_id,
        listing_version: ListingVersion::new(3).expect("版本"),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        reservation_id,
        reservation_generation: 1,
        equipment: ListingEquipmentIdentity::new("main", eid).expect("装备"),
        credential_ref: Some(Uuid::parse_str(ENVELOPE).expect("凭证")),
    }
}

fn at(value: &str) -> auction_time::Timestamp {
    auction_time::Timestamp::parse_from_rfc3339(value).expect("固定时刻")
}

#[tokio::test]
async fn postgres_accept_items_transitions_real_awaiting_deposit_and_returns_writer_l() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_first_item(pool).await;
    let repository = repository(&database).await;
    let accepted = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键"),
            vec![item(
                1,
                Uuid::parse_str(LISTING).expect("挂单"),
                "42",
                Uuid::parse_str(RESERVATION).expect("预约"),
            )],
            at("2026-01-01T03:00:00Z"),
        )
        .await
        .expect("整封接受");
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].listing_version, 4);
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("deposit_accepted".to_owned(), 4));
    let event: (String, String) = sqlx::query_as(
        "SELECT event_kind, from_state FROM public.consignment_events
         WHERE listing_id=$1 AND state_version=4",
    )
    .bind(Uuid::parse_str(LISTING).expect("挂单"))
    .fetch_one(pool)
    .await
    .expect("读取接受事件");
    assert_eq!(event, ("deposit_accepted".to_owned(), "awaiting_deposit".to_owned()));
    let receipt: serde_json::Value = sqlx::query_scalar(
        "SELECT result_body FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(Uuid::parse_str(DECIDE).expect("决定命令"))
    .fetch_one(pool)
    .await
    .expect("读取回执");
    assert_eq!(receipt["listing_changes"][0]["listing_id"], LISTING);
    assert_eq!(receipt["listing_changes"][0]["listing_version"], 4);
    let position: (String, i64) = sqlx::query_as(
        "SELECT state, version FROM public.consignment_custody_positions
         WHERE item_id=(SELECT id FROM public.consignment_deposit_items WHERE decision_id=$1)",
    )
    .bind(Uuid::parse_str(DECISION).expect("决定"))
    .fetch_one(pool)
    .await
    .expect("读取 position");
    assert_eq!(position, ("awaiting_take".to_owned(), 1));
}

#[tokio::test]
async fn postgres_accept_items_rejects_exact_reservation_expiry_with_zero_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_first_item(pool).await;
    let repository = repository(&database).await;
    let error = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键"),
            vec![item(
                1,
                Uuid::parse_str(LISTING).expect("挂单"),
                "42",
                Uuid::parse_str(RESERVATION).expect("预约"),
            )],
            at("2026-01-01T12:00:00Z"),
        )
        .await
        .expect_err("截止等号必须拒绝");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)));
    let listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("awaiting_deposit".to_owned(), 3));
    let items: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_items WHERE decision_id=$1",
    )
    .bind(Uuid::parse_str(DECISION).expect("决定"))
    .fetch_one(pool)
    .await
    .expect("读取 item");
    assert_eq!(items, 0);
    let positions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_custody_positions")
            .fetch_one(pool)
            .await
            .expect("读取 position");
    assert_eq!(positions, 0);
    let reservation: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_deposit_reservations WHERE id=$1")
            .bind(Uuid::parse_str(RESERVATION).expect("预约"))
            .fetch_one(pool)
            .await
            .expect("读取预约");
    assert_eq!(reservation, "active");
}

#[tokio::test]
async fn postgres_accept_items_accepts_one_microsecond_before_reservation_expiry() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_first_item(pool).await;
    let repository = repository(&database).await;
    let accepted = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键"),
            vec![item(
                1,
                Uuid::parse_str(LISTING).expect("挂单"),
                "42",
                Uuid::parse_str(RESERVATION).expect("预约"),
            )],
            at("2026-01-01T12:00:00Z")
                .checked_add_signed(auction_time::Duration::microseconds(-1))
                .expect("前一微秒"),
        )
        .await
        .expect("截止前一微秒接受");
    assert_eq!(accepted[0].listing_version, 4);
}

#[tokio::test]
async fn postgres_accept_items_rolls_back_first_item_when_late_item_fails() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    seed_first_item(pool).await;
    let second = seed_second_item(pool).await;
    let repository = repository(&database).await;
    let error = repository
        .test_accept_items_at(
            Uuid::parse_str(DECIDE).expect("决定命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            OriginalMailKey::new("12345", SettlementWorld::Main, "555").expect("原信键"),
            vec![
                item(
                    1,
                    Uuid::parse_str(LISTING).expect("挂单"),
                    "42",
                    Uuid::parse_str(RESERVATION).expect("预约"),
                ),
                MatchedDepositItem {
                    listing_version: ListingVersion::new(2).expect("版本"),
                    ..item(2, second.listing_id, "43", second.reservation_id)
                },
            ],
            at("2026-01-01T06:00:00Z"),
        )
        .await
        .expect_err("末件过期必须全封失败");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven)));
    let first: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取第一件挂单");
    assert_eq!(first, ("awaiting_deposit".to_owned(), 3));
    let second_listing: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(second.listing_id)
            .fetch_one(pool)
            .await
            .expect("读取第二件挂单");
    assert_eq!(second_listing, ("awaiting_deposit".to_owned(), 2));
    let items: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_deposit_items")
        .fetch_one(pool)
        .await
        .expect("读取 item");
    assert_eq!(items, 0);
    let reservations: Vec<String> =
        sqlx::query_scalar("SELECT state FROM public.consignment_deposit_reservations ORDER BY id")
            .fetch_all(pool)
            .await
            .expect("读取预约");
    assert!(reservations.iter().all(|state| state == "active"));
}
