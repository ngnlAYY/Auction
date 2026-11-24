//! 4B 季任务：预停保留既有预约、旧季关闭写原因与回执。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ReservationStore, SeasonCloseCommand,
    SeasonCloseTarget, SeasonNoticeCommand, SeasonNoticeKind, SeasonNoticeReceipt, SeasonStore,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::CalendarVersion;
use auction_domain::consignment::FrozenSeason;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::support::{self, POSITION, USER_1};
use super::helpers::{
    ISEKAI_SESSION, client_command, hash, issue_command, repository, seed_displayed_listing_eid_at,
    seed_isekai_ready, seed_listing, seed_listing_eid_at, worker_command,
};
use crate::support::TestDatabase;

const LISTING: &str = "20000000-0000-0000-0000-0000000000b7";
const SEASON: &str = "2026-isekai-1";

fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("固定挂单")
}

struct SeasonFixture {
    id: Uuid,
    start: auction_time::Timestamp,
    end: auction_time::Timestamp,
    cutoff: auction_time::Timestamp,
    calendar: [u8; 32],
}

async fn seed_season(pool: &PgPool) -> SeasonFixture {
    let id = Uuid::new_v4();
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let start = now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前");
    let end = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let cutoff = now.checked_add_signed(auction_time::Duration::days(-2)).expect("两日前");
    let calendar = [0x77_u8; 32];
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, $2, 'isekai', 'UTC', $3, $4, $5, $6, 3)",
    )
    .bind(id)
    .bind(SEASON)
    .bind(start)
    .bind(end)
    .bind(cutoff)
    .bind(calendar.as_slice())
    .execute(pool)
    .await
    .expect("写入季窗");
    SeasonFixture { id, start, end, cutoff, calendar }
}

async fn seed_open_season(pool: &PgPool) -> SeasonFixture {
    let id = Uuid::new_v4();
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let start = now.checked_add_signed(auction_time::Duration::days(-10)).expect("十日前");
    let end = now.checked_add_signed(auction_time::Duration::days(10)).expect("十日后");
    let cutoff = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let calendar = [0x78_u8; 32];
    sqlx::query(
        "INSERT INTO public.consignment_season_windows
            (id, season_id, world_id, timezone, start_utc, end_utc, admission_cutoff_utc,
             calendar_version, preclose_days)
         VALUES ($1, $2, 'isekai', 'UTC', $3, $4, $5, $6, 3)",
    )
    .bind(id)
    .bind(SEASON)
    .bind(start)
    .bind(end)
    .bind(cutoff)
    .bind(calendar.as_slice())
    .execute(pool)
    .await
    .expect("写入开放季窗");
    SeasonFixture { id, start, end, cutoff, calendar }
}

async fn seed_open_season_reservation(pool: &PgPool, season: &SeasonFixture) {
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let issued = now.checked_add_signed(auction_time::Duration::days(-5)).expect("五日前");
    let expires = now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后");
    let reservation = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开启开放季预约事务");
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1, $2, $3, 'FakeSellerOne', '12346', 'isekai', '42', $4, 1, 1,
                 $5, decode(repeat('59', 32), 'hex'), $6, $7, 'active',
                 (SELECT command_id FROM public.consignment_commands
                  WHERE listing_id=$2::uuid AND operation='register'
                  ORDER BY created_at DESC LIMIT 1), NULL, NULL)",
    )
    .bind(reservation)
    .bind(listing_id())
    .bind(Uuid::parse_str(super::helpers::USER).expect("固定卖家"))
    .bind(season.id)
    .bind(Uuid::parse_str(ISEKAI_SESSION).expect("固定会话"))
    .bind(issued)
    .bind(expires)
    .execute(&mut *transaction)
    .await
    .expect("写入开放季预约");
    sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('isekai', '42', 1, $1, NULL, $2, $1)",
    )
    .bind(reservation)
    .bind(issued)
    .execute(&mut *transaction)
    .await
    .expect("写入开放季占用");
    transaction.commit().await.expect("提交开放季预约");
}

async fn seed_accepted_in_season(pool: &PgPool, season: &SeasonFixture) {
    let position_with_season = support::accepted_envelope()
        .replace("'main'", "'isekai'")
        .replace("|main|", "|isekai|")
        .replace(
            " 'isekai', '42',\n                 NULL, 'awaiting_take'",
            &format!(" 'isekai', '42',\n                 '{}', 'awaiting_take'", season.id),
        );
    let mut transaction = pool.begin().await.expect("开启季接受夹具事务");
    sqlx::raw_sql(sqlx::AssertSqlSafe(position_with_season))
        .execute(&mut *transaction)
        .await
        .expect("写入季内已接受夹具");
    transaction.commit().await.expect("提交季内已接受夹具");
}

async fn seed_season_reservation(pool: &PgPool, season: &SeasonFixture) {
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let issued = now.checked_add_signed(auction_time::Duration::days(-5)).expect("五日前");
    let expires = season.end;
    let reservation = Uuid::new_v4();
    let mut transaction = pool.begin().await.expect("开启季预约事务");
    sqlx::query(
        "INSERT INTO public.consignment_deposit_reservations
            (id, listing_id, seller_user_id, verified_sender, bot_uid, world_id, canonical_eid,
             season_window_id, generation, service_epoch, session_id, token_digest, issued_at,
             expires_at, state, last_command_id, retired_at, reason)
         VALUES ($1, $2, $3, 'FakeSellerOne', '12346', 'isekai', '42', $4, 1, 1,
                 $5, decode(repeat('58', 32), 'hex'), $6, $7, 'active',
                 (SELECT command_id FROM public.consignment_commands
                  WHERE listing_id=$2::uuid AND operation='register'
                  ORDER BY created_at DESC LIMIT 1), NULL, NULL)",
    )
    .bind(reservation)
    .bind(listing_id())
    .bind(Uuid::parse_str(super::helpers::USER).expect("固定卖家"))
    .bind(season.id)
    .bind(Uuid::parse_str(ISEKAI_SESSION).expect("固定会话"))
    .bind(issued)
    .bind(expires)
    .execute(&mut *transaction)
    .await
    .expect("写入季预约");
    sqlx::query(
        "INSERT INTO public.consignment_equipment_claims
            (world_id, canonical_eid, revision, reservation_id, position_id, changed_at,
             last_fact_id)
         VALUES ('isekai', '42', 1, $1, NULL, $2, $1)",
    )
    .bind(reservation)
    .bind(issued)
    .execute(&mut *transaction)
    .await
    .expect("写入季占用");
    transaction.commit().await.expect("提交季预约");
}

fn season_command(
    command_id: Uuid,
    season: &SeasonFixture,
    target: SeasonCloseTarget,
) -> SeasonCloseCommand {
    SeasonCloseCommand {
        command_id: auction_domain::CommandId::from_uuid(command_id),
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: SEASON.to_owned(),
            start_utc: season.start,
            end_utc: season.end,
            admission_cutoff_utc: season.cutoff,
            calendar_version: CalendarVersion::from_bytes(season.calendar),
            preclose_days: 3,
        },
        target,
        request_hash: hash(0x71),
    }
}

#[tokio::test]
async fn postgres_season_cutoff_preserves_valid_prior_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), SettlementWorld::Isekai).await;
    seed_isekai_ready(pool).await;
    let season = seed_open_season(pool).await;
    seed_open_season_reservation(pool, &season).await;
    let repository = repository(&database).await;
    let error = repository
        .issue_reservation(issue_command(client_command(70), listing_id(), 2, 0x72))
        .await
        .expect_err("预停后不能换发新预约");
    assert!(
        matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::SeasonClosed)),
        "{error:?}"
    );
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_reservations
         WHERE listing_id=$1 AND state='active'",
    )
    .bind(listing_id())
    .fetch_one(pool)
    .await
    .expect("读取既有预约");
    assert_eq!(active, 1, "先前有效预约不能被预停清退");
}

#[tokio::test]
async fn postgres_season_notice_deduplicates_and_replays() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let season = seed_open_season(pool).await;
    seed_accepted_in_season(pool, &season).await;
    let repository = repository(&database).await;
    let command = SeasonNoticeCommand {
        command_id: worker_command(74),
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: SEASON.to_owned(),
            start_utc: season.start,
            end_utc: season.end,
            admission_cutoff_utc: season.cutoff,
            calendar_version: CalendarVersion::from_bytes(season.calendar),
            preclose_days: 3,
        },
        position_id: Uuid::parse_str(POSITION).expect("position"),
        seller_user_id: auction_domain::UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        notice_kind: SeasonNoticeKind::PrecloseWithdrawal,
        request_hash: hash(0x93),
    };
    let first: SeasonNoticeReceipt =
        repository.queue_season_notice(command.clone()).await.expect("排队通知");
    let replay = repository.queue_season_notice(command).await.expect("重放通知");
    assert_eq!(replay, first);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_season_notices
         WHERE position_id=$1 AND seller_user_id=$2 AND kind='preclose_withdrawal'",
    )
    .bind(Uuid::parse_str(POSITION).expect("position"))
    .bind(Uuid::parse_str(USER_1).expect("卖家"))
    .fetch_one(pool)
    .await
    .expect("读取通知数");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn postgres_season_notice_deduplicates_across_different_commands() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let season = seed_open_season(pool).await;
    seed_accepted_in_season(pool, &season).await;
    let repository = repository(&database).await;
    let command = |command_id| SeasonNoticeCommand {
        command_id,
        season: FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: SEASON.to_owned(),
            start_utc: season.start,
            end_utc: season.end,
            admission_cutoff_utc: season.cutoff,
            calendar_version: CalendarVersion::from_bytes(season.calendar),
            preclose_days: 3,
        },
        position_id: Uuid::parse_str(POSITION).expect("position"),
        seller_user_id: auction_domain::UserId::from_uuid(Uuid::parse_str(USER_1).expect("卖家")),
        notice_kind: SeasonNoticeKind::PrecloseWithdrawal,
        request_hash: hash(0x94),
    };
    let first = repository
        .queue_season_notice(command(worker_command(75)))
        .await
        .expect("首个命令排队通知");
    let second = repository
        .queue_season_notice(command(worker_command(76)))
        .await
        .expect("不同命令的同一通知");
    assert_eq!(second.work_id, first.work_id, "回执必须指向已有通知");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_season_notices
         WHERE position_id=$1 AND seller_user_id=$2 AND kind='preclose_withdrawal'",
    )
    .bind(Uuid::parse_str(POSITION).expect("position"))
    .bind(Uuid::parse_str(USER_1).expect("卖家"))
    .fetch_one(pool)
    .await
    .expect("读取通知数");
    assert_eq!(count, 1, "不同命令不得新建同一业务通知");
}

#[tokio::test]
async fn postgres_season_close_writes_reason_and_closed_ids() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), SettlementWorld::Isekai).await;
    seed_isekai_ready(pool).await;
    let season = seed_open_season(pool).await;
    seed_season_reservation(pool, &season).await;
    let repository = repository(&database).await;
    let command = season_command(
        worker_command(71).as_uuid(),
        &season,
        SeasonCloseTarget::World(SettlementWorld::Isekai),
    );
    let receipt =
        repository.test_close_season_at(command.clone(), season.end).await.expect("关闭旧季");
    assert_eq!(receipt.closed_listing_ids, vec![listing_id()]);
    let replay = repository.test_close_season_at(command, season.end).await.expect("重放");
    assert_eq!(replay, receipt);
    let listing: (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("closed".to_owned(), Some("season_closed".to_owned())));
    let event: String = sqlx::query_scalar(
        "SELECT event_kind FROM public.consignment_events
         WHERE listing_id=$1 AND reason_code='season_closed' ORDER BY state_version DESC LIMIT 1",
    )
    .bind(listing_id())
    .fetch_one(pool)
    .await
    .expect("读取季关闭事件");
    assert_eq!(event, "season_closed");
}

#[tokio::test]
async fn postgres_season_close_rejects_early_and_unknown_window() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), SettlementWorld::Isekai).await;
    seed_isekai_ready(pool).await;
    let season = seed_season(pool).await;
    let repository = repository(&database).await;
    let mut early = season_command(
        worker_command(72).as_uuid(),
        &season,
        SeasonCloseTarget::World(SettlementWorld::Isekai),
    );
    early.season.end_utc = early
        .season
        .end_utc
        .checked_add_signed(auction_time::Duration::hours(2))
        .expect("两小时后");
    let error = repository.close_season(early).await.expect_err("未到冻结界");
    assert!(matches!(
        error,
        ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch)
            | ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)
    ));
    let missing = season_command(
        worker_command(73).as_uuid(),
        &season,
        SeasonCloseTarget::Listing(Uuid::new_v4()),
    );
    let error = repository.close_season(missing).await.expect_err("未知目标");
    assert!(matches!(
        error,
        ConsignmentError::Deposit(DepositBusinessRejection::SourceMismatch)
            | ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)
    ));
}

#[tokio::test]
async fn postgres_season_close_world_scope_uses_registration_time_and_world() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let season = seed_season(pool).await;
    let displayed = Uuid::new_v4();
    seed_displayed_listing_eid_at(
        pool,
        displayed,
        SettlementWorld::Isekai,
        season.end.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前"),
        auction_time::Timestamp::parse_from_rfc3339("2099-01-01T00:00:00Z").expect("远期"),
        "42",
    )
    .await;
    let new_season = Uuid::new_v4();
    seed_listing_eid_at(
        pool,
        new_season,
        SettlementWorld::Isekai,
        season.end.checked_add_signed(auction_time::Duration::minutes(30)).expect("新季登记"),
        auction_time::Timestamp::parse_from_rfc3339("2099-02-01T00:00:00Z").expect("远期"),
        "43",
    )
    .await;
    let main = Uuid::new_v4();
    seed_listing(pool, main, SettlementWorld::Main).await;
    let repository = repository(&database).await;
    let receipt = repository
        .close_season(season_command(
            worker_command(77).as_uuid(),
            &season,
            SeasonCloseTarget::World(SettlementWorld::Isekai),
        ))
        .await
        .expect("关闭旧季");
    assert_eq!(receipt.closed_listing_ids, vec![displayed]);
    let states: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM public.consignment_listings
         WHERE id IN ($1, $2, $3) ORDER BY id",
    )
    .bind(displayed)
    .bind(new_season)
    .bind(main)
    .fetch_all(pool)
    .await
    .expect("读取挂单状态");
    assert!(states.contains(&(displayed, "closed".to_owned())));
    assert!(states.contains(&(new_season, "awaiting_deposit".to_owned())));
    assert!(states.contains(&(main, "awaiting_deposit".to_owned())));
}
