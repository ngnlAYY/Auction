//! 4B 截止、季界与截止钳制的等号边界。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DepositRead, OwnedDepositInstructions,
    SeasonCloseCommand, SeasonCloseTarget,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{CalendarVersion, FrozenSeason};
use sqlx::PgPool;
use uuid::Uuid;

use super::helpers::{
    LISTING, client_command, expire_command, hash, issue_command, repository,
    seed_active_reservation, seed_isekai_ready, seed_listing_at, seed_listing_eid_at,
    seed_mail_ready, worker_command,
};
use crate::support::TestDatabase;

struct Season {
    start: auction_time::Timestamp,
    end: auction_time::Timestamp,
    cutoff: auction_time::Timestamp,
    calendar: [u8; 32],
}

async fn seed_reservation_secret(
    pool: &PgPool,
    reservation_id: Uuid,
    issued_at: auction_time::Timestamp,
    token: &str,
) {
    sqlx::query(
        "INSERT INTO public.consignment_reservation_secrets
            (reservation_id, generation, seller_user_id, ciphertext, nonce, encryption_key_id,
             fingerprint, created_at, updated_at, retired_at)
         VALUES ($1, 1, $2, $3, $4, 'test-key', $5, $6, $6, NULL)",
    )
    .bind(reservation_id)
    .bind(Uuid::parse_str(super::helpers::USER).expect("卖家"))
    .bind(token.as_bytes())
    .bind([7_u8; 12].as_slice())
    .bind("a".repeat(64))
    .bind(issued_at)
    .execute(pool)
    .await
    .expect("写入预约密文");
}

async fn seed_season(
    pool: &PgPool,
    start: auction_time::Timestamp,
    end: auction_time::Timestamp,
    cutoff: auction_time::Timestamp,
) -> Season {
    let calendar = [0x71_u8; 32];
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, '2026-isekai-boundary', 'isekai', 'UTC', $2, $3, $4, $5, 3)",
    )
    .bind(Uuid::new_v4())
    .bind(start)
    .bind(end)
    .bind(cutoff)
    .bind(calendar.as_slice())
    .execute(pool)
    .await
    .expect("写入季窗");
    Season { start, end, cutoff, calendar }
}

fn close_command(seed: u128, season: &Season) -> SeasonCloseCommand {
    SeasonCloseCommand {
        command_id: worker_command(seed),
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: "2026-isekai-boundary".to_owned(),
            start_utc: season.start,
            end_utc: season.end,
            admission_cutoff_utc: season.cutoff,
            calendar_version: CalendarVersion::from_bytes(season.calendar),
            preclose_days: 3,
        },
        target: SeasonCloseTarget::World(SettlementWorld::Isekai),
        request_hash: hash(0xb1),
    }
}

#[tokio::test]
async fn postgres_season_close_rejects_one_microsecond_before_and_accepts_at_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_isekai_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let at = now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后");
    let season = seed_season(
        pool,
        at.checked_add_signed(auction_time::Duration::days(-1)).expect("一日前"),
        at,
        at.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
    )
    .await;
    seed_listing_at(
        pool,
        Uuid::parse_str(LISTING).expect("固定挂单"),
        SettlementWorld::Isekai,
        at.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前"),
        at.checked_add_signed(auction_time::Duration::days(1)).expect("一日后"),
    )
    .await;
    let repository = repository(&database).await;
    let early = repository
        .test_close_season_at(
            close_command(90, &season),
            at.checked_add_signed(auction_time::Duration::microseconds(-1)).expect("前一微秒"),
        )
        .await
        .expect_err("季末前一微秒必须拒绝");
    assert!(matches!(early, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)));
    let receipt = repository
        .test_close_season_at(close_command(91, &season), at)
        .await
        .expect("季末等号关闭");
    assert_eq!(receipt.closed_listing_ids, vec![Uuid::parse_str(LISTING).expect("挂单")]);
    let listing: (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(LISTING).expect("挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("closed".to_owned(), Some("season_closed".to_owned())));
}

#[tokio::test]
async fn postgres_issue_reservation_uses_earliest_deadline_ttl_and_season_end() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    // 夹具的 updated_at 取真实事务时刻，而触发器拒绝早于它的迁移时刻（updated_at 不得回退）。
    // 季末段还要再克隆一个库并播种，真实耗时在重负载下可超过秒级，所以固定时刻留足余量，
    // 保证它晚于所有夹具行的真实写入时刻。
    let at = now.checked_add_signed(auction_time::Duration::minutes(5)).expect("五分钟后");
    let repository = repository(&database).await;

    let ttl_listing = Uuid::new_v4();
    seed_listing_eid_at(
        pool,
        ttl_listing,
        SettlementWorld::Main,
        at.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        at.checked_add_signed(auction_time::Duration::hours(25)).expect("二十五小时后"),
        "52",
    )
    .await;
    let ttl = repository
        .test_issue_reservation_at(issue_command(client_command(100), ttl_listing, 2, 0xc1), at)
        .await
        .expect("TTL 为最早截止");
    assert_eq!(ttl.expires_at, at + auction_time::Duration::hours(24));

    let deadline_listing = Uuid::new_v4();
    seed_listing_eid_at(
        pool,
        deadline_listing,
        SettlementWorld::Main,
        at.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        at.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后"),
        "53",
    )
    .await;
    let deadline = repository
        .test_issue_reservation_at(
            issue_command(client_command(101), deadline_listing, 2, 0xc2),
            at,
        )
        .await
        .expect("待寄截止为最早截止");
    assert_eq!(deadline.expires_at, at + auction_time::Duration::hours(1));

    let season_database = TestDatabase::create().await.expect("克隆季模板库");
    let season_maintenance = season_database.maintenance_store().await.expect("季维护连接");
    let season_pool = season_maintenance.postgres_pool();
    seed_isekai_ready(season_pool).await;
    let season_listing = Uuid::new_v4();
    seed_listing_eid_at(
        season_pool,
        season_listing,
        SettlementWorld::Isekai,
        at.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        at.checked_add_signed(auction_time::Duration::hours(25)).expect("二十五小时后"),
        "54",
    )
    .await;
    seed_season(
        season_pool,
        at.checked_add_signed(auction_time::Duration::days(-1)).expect("一日前"),
        at.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后"),
        at.checked_add_signed(auction_time::Duration::minutes(30)).expect("三十分钟后"),
    )
    .await;
    let season_repository = super::helpers::repository(&season_database).await;
    let season = season_repository
        .test_issue_reservation_at(issue_command(client_command(102), season_listing, 2, 0xc3), at)
        .await
        .expect("季末为最早截止");
    assert_eq!(season.expires_at, at + auction_time::Duration::hours(1));
}

#[tokio::test]
async fn postgres_issue_reservation_rejects_exact_listing_deadline_and_accepts_one_microsecond_before()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    // 与上一条同理：固定时刻须晚于夹具行的真实写入时刻，重负载下半秒余量不够。
    let deadline = now.checked_add_signed(auction_time::Duration::minutes(5)).expect("五分钟后");
    let at =
        deadline.checked_add_signed(auction_time::Duration::microseconds(-1)).expect("前一微秒");
    let repository = repository(&database).await;
    let valid_listing = Uuid::new_v4();
    seed_listing_eid_at(
        pool,
        valid_listing,
        SettlementWorld::Main,
        now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        deadline,
        "55",
    )
    .await;
    let receipt = repository
        .test_issue_reservation_at(issue_command(client_command(103), valid_listing, 2, 0xc4), at)
        .await
        .expect("截止前一微秒仍有效");
    assert_eq!(receipt.expires_at, deadline);

    let expired_listing = Uuid::new_v4();
    seed_listing_eid_at(
        pool,
        expired_listing,
        SettlementWorld::Main,
        now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        deadline,
        "56",
    )
    .await;
    let expired = repository
        .test_issue_reservation_at(
            issue_command(client_command(104), expired_listing, 2, 0xc5),
            deadline,
        )
        .await
        .expect_err("截止等号失效");
    assert!(matches!(
        expired,
        ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)
    ));
}

#[tokio::test]
async fn postgres_instructions_hide_active_but_expired_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let listing_id = Uuid::new_v4();
    super::helpers::seed_listing(pool, listing_id, SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(
        pool,
        listing_id,
        reservation_id,
        1,
        now.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前"),
        now.checked_add_signed(auction_time::Duration::seconds(-1)).expect("一秒前"),
    )
    .await;
    let token = auction_domain::consignment::deposit::format_token([0x33; 32]);
    let issued = now.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前");
    seed_reservation_secret(pool, reservation_id, issued, token.as_str()).await;
    let repository = repository(&database).await;
    let owner =
        auction_domain::UserId::from_uuid(Uuid::parse_str(super::helpers::USER).expect("固定卖家"));
    assert_eq!(
        repository.instructions(owner, listing_id).await.expect("读取指引"),
        Some(auction_application::consignment::OwnedDepositInstructions::None)
    );
}

#[tokio::test]
async fn postgres_instructions_boundary_accepts_one_microsecond_before_and_rejects_expiry() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let listing_id = Uuid::new_v4();
    super::helpers::seed_listing(pool, listing_id, SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let at = now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后");
    let expires_at =
        at.checked_add_signed(auction_time::Duration::microseconds(1)).expect("一微秒后");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(
        pool,
        listing_id,
        reservation_id,
        1,
        now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        expires_at,
    )
    .await;
    let token = auction_domain::consignment::deposit::format_token([0x44; 32]);
    seed_reservation_secret(
        pool,
        reservation_id,
        now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前"),
        token.as_str(),
    )
    .await;
    let repository = repository(&database).await;
    let owner =
        auction_domain::UserId::from_uuid(Uuid::parse_str(super::helpers::USER).expect("固定卖家"));
    let before =
        repository.test_instructions_at(owner, listing_id, at).await.expect("读取截止前一微秒");
    let Some(OwnedDepositInstructions::Active(active)) = before else {
        panic!("截止前一微秒必须返回活动指引");
    };
    assert_eq!(active.body, token);
    assert_eq!(
        repository.test_instructions_at(owner, listing_id, expires_at).await.expect("读取截止等号"),
        Some(OwnedDepositInstructions::None)
    );
}

#[tokio::test]
async fn postgres_expire_reservation_rejects_one_microsecond_before_and_retires_at_expiry() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let listing_id = Uuid::new_v4();
    super::helpers::seed_listing(pool, listing_id, SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let issued = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let expires_at = now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(pool, listing_id, reservation_id, 1, issued, expires_at).await;
    let repository = repository(&database).await;

    // 截止前一微秒：预约仍有效，到期退休必须拒绝，预约与强占用保持不变（D9 §2.1：截止到达才失效）。
    let before =
        expires_at.checked_add_signed(auction_time::Duration::microseconds(-1)).expect("前一微秒");
    let early = repository
        .test_expire_reservation_at(
            expire_command(worker_command(0x201), reservation_id, 1, expires_at),
            before,
        )
        .await
        .expect_err("截止前不得按到期退休");
    assert!(matches!(early, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)));
    let still: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id = $1",
    )
    .bind(reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(still, ("active".to_owned(), None));
    let holder: Option<Uuid> = sqlx::query_scalar(
        "SELECT reservation_id FROM public.consignment_equipment_claims
         WHERE world_id = 'main' AND canonical_eid = '42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取强占用");
    assert_eq!(holder, Some(reservation_id));

    // 截止等号：预约已失效，可以按到期退休并释放强占用。
    let retired = repository
        .test_expire_reservation_at(
            expire_command(worker_command(0x202), reservation_id, 1, expires_at),
            expires_at,
        )
        .await
        .expect("截止等号按到期退休");
    assert_eq!(retired.reservation_id, reservation_id);
    let after: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id = $1",
    )
    .bind(reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(after, ("retired".to_owned(), Some("expired".to_owned())));
}
