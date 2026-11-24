//! 4B 预约端口：签发、换发、退休、重放与确定拒绝。

use auction_application::consignment::{
    ConsignmentError, CustodyControlStore, DepositBusinessRejection, DepositRead,
    ListingCommandStore, RepriceListingCommand, ReservationInvalidation, ReservationStore,
    SellerListingTarget,
};
use auction_domain::UserId;
use auction_domain::consignment::{FeeBps, ListingPrice, ListingState};
use auction_storage::DepositSettings;
use uuid::Uuid;

use super::helpers::{
    USER, client_command, command_service, expire_command, hash, issue_command, repository,
    repository_with_settings, seed_active_reservation, seed_listing, seed_listing_eid_at,
    seed_mail_ready, worker_command,
};
use crate::support::TestDatabase;

fn listing_id() -> Uuid {
    Uuid::parse_str(super::helpers::LISTING).expect("固定挂单")
}

fn seller() -> UserId {
    UserId::from_uuid(Uuid::parse_str(USER).expect("固定卖家"))
}

fn version(value: u64) -> auction_domain::consignment::ListingVersion {
    auction_domain::consignment::ListingVersion::new(value).expect("测试版本")
}

#[tokio::test]
async fn postgres_deposit_issue_reservation_replays_and_conflicts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let command_id = client_command(1);
    let command = issue_command(command_id, listing_id(), 2, 0x21);
    let first = repository.issue_reservation(command.clone()).await.expect("首次签发");
    assert_eq!(first.listing_version.value(), 3);
    assert_eq!(first.state, ListingState::AwaitingDeposit);
    let first_body: String = sqlx::query_scalar(
        "SELECT result_body::text FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(command_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("读取首发回执");
    let replay = repository.issue_reservation(command.clone()).await.expect("同请求重放");
    assert_eq!(replay, first);
    let replay_body: String = sqlx::query_scalar(
        "SELECT result_body::text FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(command_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("读取重放回执");
    assert_eq!(replay_body, first_body, "重放必须逐字节保留原回执");
    let conflict = repository
        .issue_reservation(issue_command(command_id, listing_id(), 2, 0x22))
        .await
        .expect_err("同命令异摘要");
    assert!(matches!(conflict, ConsignmentError::CommandIdConflict));
    let stale = repository
        .issue_reservation(issue_command(client_command(2), listing_id(), 2, 0x23))
        .await
        .expect_err("旧版本签发");
    assert!(matches!(stale, ConsignmentError::Deposit(DepositBusinessRejection::StaleVersion)));
    let state: (String, i64) =
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(state, ("awaiting_deposit".to_owned(), 3));
    let reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_reservations WHERE listing_id=$1",
    )
    .bind(listing_id())
    .fetch_one(pool)
    .await
    .expect("读取预约数");
    assert_eq!(reservations, 1, "重放与冲突不得新增预约");
    let commands: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands WHERE command_id=$1")
            .bind(command_id.as_uuid())
            .fetch_one(pool)
            .await
            .expect("读取命令行");
    assert_eq!(commands, 1);
}

#[tokio::test]
async fn postgres_deposit_issue_reservation_replays_after_soft_close() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let service = command_service(&repository);
    let command_id = client_command(130);
    let first = service
        .issue_reservation(seller(), command_id, listing_id(), version(2), None)
        .await
        .expect("签发预约");
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands")
        .fetch_one(pool)
        .await
        .expect("关闭前命令数");

    sqlx::query("UPDATE public.consignment_lookup_control SET admitting=false WHERE id=1")
        .execute(pool)
        .await
        .expect("软关闭新承诺");

    // 不变量 1、D9 §6.3：关闭前已接受的同 ID 同请求仍返回首次回执。
    let replay = service
        .replay_issue_reservation(seller(), command_id, listing_id(), version(2), None)
        .await
        .expect("关闭后重放");
    assert_eq!(replay, Some(first), "关闭后必须返回原回执");
    let fresh = service
        .replay_issue_reservation(seller(), client_command(131), listing_id(), version(2), None)
        .await
        .expect("关闭后新 ID 只读查询");
    assert_eq!(fresh, None, "关闭后未落库的新 ID 必须为空");
    let conflict = service
        .replay_issue_reservation(seller(), command_id, listing_id(), version(3), None)
        .await
        .expect_err("同命令异请求");
    assert!(matches!(conflict, ConsignmentError::CommandIdConflict));
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands")
        .fetch_one(pool)
        .await
        .expect("关闭后命令数");
    assert_eq!(after, before, "重放、新 ID 与冲突都不得新增命令行");
}

#[tokio::test]
async fn postgres_deposit_reservation_ttl_uses_configured_hours_and_clamps_to_deposit_deadline() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let created = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("登记时刻");
    let deposit_deadline =
        now.checked_add_signed(auction_time::Duration::hours(10)).expect("较早待寄截止");
    let far_deadline =
        now.checked_add_signed(auction_time::Duration::hours(72)).expect("较晚待寄截止");
    let first_listing = Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_00c1);
    let second_listing = Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_00c2);
    seed_listing_eid_at(
        pool,
        first_listing,
        auction_domain::SettlementWorld::Main,
        created,
        far_deadline,
        "42",
    )
    .await;
    seed_listing_eid_at(
        pool,
        second_listing,
        auction_domain::SettlementWorld::Main,
        created,
        deposit_deadline,
        "43",
    )
    .await;
    seed_mail_ready(pool).await;
    // 与缺省 24 不同的 36 小时；硬编码缺省值必须让本用例失败。
    let settings = DepositSettings::new(
        FeeBps::new(500).expect("测试费率"),
        ListingPrice::new(ListingPrice::MIN).expect("测试下限"),
        ListingPrice::new(ListingPrice::MAX).expect("测试上限"),
        36,
        100,
    );
    let repository = repository_with_settings(&database, settings).await;
    let service = command_service(&repository);

    let first = service
        .issue_reservation(seller(), client_command(140), first_listing, version(2), None)
        .await
        .expect("按配置 TTL 签发");
    let (issued_at, expires_at): (auction_time::Timestamp, auction_time::Timestamp) =
        sqlx::query_as(
            "SELECT issued_at, expires_at FROM public.consignment_deposit_reservations WHERE id=$1",
        )
        .bind(first.reservation_id)
        .fetch_one(pool)
        .await
        .expect("读取预约时刻");
    assert_eq!(
        expires_at,
        issued_at.checked_add_signed(auction_time::Duration::hours(36)).expect("36 小时截止"),
        "预约截止必须是签发时刻加配置的 36 小时"
    );
    assert_eq!(first.expires_at, expires_at);

    let second = service
        .issue_reservation(seller(), client_command(141), second_listing, version(2), None)
        .await
        .expect("按待寄截止签发");
    assert_eq!(second.expires_at, deposit_deadline, "早于 TTL 时取待寄截止");
}

#[tokio::test]
async fn postgres_deposit_expire_reservation_retires_and_releases_claim() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let issued = now.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前");
    let expired = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(pool, listing_id(), reservation_id, 1, issued, expired).await;
    let repository = repository(&database).await;
    let command_id = worker_command(1);
    let command = expire_command(command_id, reservation_id, 1, expired);
    let receipt = repository.expire_reservation(command.clone()).await.expect("到期退休");
    assert_eq!(receipt.reservation_id, reservation_id);
    let replay = repository.expire_reservation(command).await.expect("同请求重放");
    assert_eq!(replay, receipt);
    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id=$1",
    )
    .bind(reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(state, ("retired".to_owned(), Some("expired".to_owned())));
    let holder: Option<Uuid> = sqlx::query_scalar(
        "SELECT reservation_id FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取强占用");
    assert_eq!(holder, None);
}

/// 回归：预约退休后占用行保留为空行，同一挂单仍可签发新预约（反例见 busy.rs 的活动预约阻止用例）。
#[tokio::test]
async fn postgres_deposit_issue_succeeds_after_reservation_expired() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let issued = now.checked_add_signed(auction_time::Duration::hours(-2)).expect("两小时前");
    let expired = now.checked_add_signed(auction_time::Duration::hours(-1)).expect("一小时前");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(pool, listing_id(), reservation_id, 1, issued, expired).await;
    let repository = repository(&database).await;
    repository
        .expire_reservation(expire_command(worker_command(1), reservation_id, 1, expired))
        .await
        .expect("到期退休");
    let listing_version: i64 =
        sqlx::query_scalar("SELECT state_version FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(pool)
            .await
            .expect("读取挂单版本");
    let listing_version = u64::try_from(listing_version).expect("版本非负");
    let reissued = repository
        .issue_reservation(issue_command(client_command(2), listing_id(), listing_version, 0x22))
        .await
        .expect("已释放的占用行不得阻止同件重新签发");
    assert_eq!(reissued.state, ListingState::AwaitingDeposit);
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_deposit_reservations
         WHERE listing_id=$1 AND state='active'",
    )
    .bind(listing_id())
    .fetch_one(pool)
    .await
    .expect("读取活动预约");
    assert_eq!(active, 1);
}

#[tokio::test]
async fn postgres_deposit_expire_cannot_declare_cancelled() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let now: auction_time::Timestamp =
        sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(pool).await.expect("数据库时钟");
    let reservation_id = Uuid::new_v4();
    seed_active_reservation(
        pool,
        listing_id(),
        reservation_id,
        1,
        now,
        now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后"),
    )
    .await;
    let repository = repository(&database).await;
    let mut command = expire_command(
        worker_command(2),
        reservation_id,
        1,
        now.checked_add_signed(auction_time::Duration::hours(1)).expect("一小时后"),
    );
    command.invalidation = ReservationInvalidation::Cancelled { command_id: client_command(7) };
    let error = repository.expire_reservation(command).await.expect_err("取消不能单独退休");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)));
    let state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_deposit_reservations WHERE id=$1")
            .bind(reservation_id)
            .fetch_one(pool)
            .await
            .expect("读取预约");
    assert_eq!(state, "active");
}

#[tokio::test]
async fn postgres_deposit_issue_rejects_expired_listing_and_unready_mail() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET admitting=true, worker_ready_until=clock_timestamp() + interval '1 hour'
         WHERE id=1",
    )
    .execute(pool)
    .await
    .expect("打开解析闸门");
    let repository = repository(&database).await;
    let error = repository
        .issue_reservation(issue_command(client_command(3), listing_id(), 2, 0x31))
        .await
        .expect_err("缺少收件就绪");
    assert!(
        matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::MailUnavailable)),
        "{error:?}"
    );
    seed_mail_ready(pool).await;
    let receipt = repository
        .issue_reservation(issue_command(client_command(4), listing_id(), 2, 0x32))
        .await
        .expect("就绪后可签发");
    assert_eq!(receipt.listing_version.value(), 3);
}

#[tokio::test]
async fn postgres_issue_reservation_replay_after_listing_closed_returns_first_receipt() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let command = issue_command(client_command(120), listing_id(), 2, 0xf1);
    let first = repository.issue_reservation(command.clone()).await.expect("签发预约");
    repository
        .request_withdrawal(auction_application::consignment::WithdrawDepositCommand {
            command_id: client_command(121),
            seller_user_id: UserId::from_uuid(Uuid::parse_str(USER).expect("卖家")),
            listing_id: listing_id(),
            expected_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
            request_hash: hash(0xf2),
            request_id: None,
        })
        .await
        .expect("关闭挂单");
    let replay = repository.issue_reservation(command).await.expect("状态改变后重放");
    assert_eq!(replay, first, "重放必须返回首次回执");
    let listing: (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("closed".to_owned(), Some("seller_cancelled".to_owned())));
}

#[tokio::test]
async fn postgres_deposit_reprice_keeps_code_and_reissue_retires_old_secret() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let first = repository
        .issue_reservation(issue_command(client_command(80), listing_id(), 2, 0xb0))
        .await
        .expect("首次签发");
    let seller = UserId::from_uuid(Uuid::parse_str(USER).expect("固定卖家"));
    let before = repository.instructions(seller, listing_id()).await.expect("读取改价前指引");
    let price = ListingPrice::new(202).expect("价格");
    repository
        .store()
        .reprice(RepriceListingCommand {
            request_hash: hash(0xb1),
            target: SellerListingTarget {
                command_id: client_command(81),
                seller_user_id: seller,
                listing_id: listing_id(),
                expected_version: auction_domain::consignment::ListingVersion::new(3)
                    .expect("版本"),
            },
            request_id: None,
            price,
            enabled: true,
        })
        .await
        .expect("改价")
        .into_result()
        .expect("改价成功");
    let after = repository.instructions(seller, listing_id()).await.expect("读取改价后指引");
    assert_eq!(after, before, "改价不得换码或改预约");
    let second = repository
        .issue_reservation(issue_command(client_command(82), listing_id(), 4, 0xb2))
        .await
        .expect("换发预约");
    assert_ne!(second.reservation_id, first.reservation_id);
    let active: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations
         WHERE id=$1",
    )
    .bind(first.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取旧预约");
    assert_eq!(active, ("retired".to_owned(), Some("reissued".to_owned())));
    let old_secret: (Option<Vec<u8>>, bool) = sqlx::query_as(
        "SELECT ciphertext, retired_at IS NOT NULL
         FROM public.consignment_reservation_secrets WHERE reservation_id=$1",
    )
    .bind(first.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取旧密文");
    assert_eq!(old_secret, (None, true));
    let generations: Vec<i64> = sqlx::query_scalar(
        "SELECT generation FROM public.consignment_deposit_reservations
         WHERE listing_id=$1 ORDER BY generation",
    )
    .bind(listing_id())
    .fetch_all(pool)
    .await
    .expect("读取预约代");
    assert_eq!(generations, vec![1, 2]);
}
