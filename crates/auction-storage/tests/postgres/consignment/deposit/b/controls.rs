//! 4B 代管控制：撤回意愿、未代管关闭、Admin 止售与解除。

use auction_application::consignment::{
    AdminDelistListingCommand, ConsignmentError, CustodyControlStore, DepositBusinessRejection,
    ListingCommandStore, ListingDelistReason, ReservationStore, SellerListingTarget,
    SetOperationsHoldCommand, WithdrawDisposition, WithdrawListingCommand,
};
use auction_domain::consignment::ListingState;
use uuid::Uuid;

use super::super::support::{self, USER_1};
use super::helpers::{
    client_command, command_service, hash, issue_command, repository, seed_listing, seed_mail_ready,
};
use crate::support::TestDatabase;

fn listing_id() -> Uuid {
    Uuid::parse_str(super::helpers::LISTING).expect("固定挂单")
}

#[tokio::test]
async fn postgres_deposit_admin_delist_replays_after_soft_close() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let service = command_service(&repository);
    let reason = ListingDelistReason::new("fixture delist".to_owned()).expect("说明");
    let command_id = client_command(160);
    let expected_version = auction_domain::consignment::ListingVersion::new(2).expect("版本");
    let first = service
        .set_operations_hold(
            "fake-admin".to_owned(),
            command_id,
            listing_id(),
            expected_version,
            reason.clone(),
            None,
        )
        .await
        .expect("Admin 下架");
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_commands")
        .fetch_one(pool)
        .await
        .expect("关闭前命令数");

    // 不变量 1、D9 §6.3：软关闭只截止新承诺；同一命令重放仍返回首次回执。
    let replay = service
        .replay_set_operations_hold(
            "fake-admin".to_owned(),
            command_id,
            listing_id(),
            expected_version,
            reason.clone(),
            None,
        )
        .await
        .expect("关闭后重放");
    assert_eq!(replay, Some(first), "关闭后必须返回原回执");
    let fresh = service
        .replay_set_operations_hold(
            "fake-admin".to_owned(),
            client_command(161),
            listing_id(),
            expected_version,
            reason.clone(),
            None,
        )
        .await
        .expect("关闭后新 ID 只读查询");
    assert_eq!(fresh, None, "关闭后未落库的新 ID 必须为空");
    let conflict = service
        .replay_set_operations_hold(
            "fake-admin".to_owned(),
            command_id,
            listing_id(),
            auction_domain::consignment::ListingVersion::new(3).expect("版本"),
            reason,
            None,
        )
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
async fn postgres_deposit_withdraw_unmanaged_closes_and_retires_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let issued = repository
        .issue_reservation(issue_command(client_command(10), listing_id(), 2, 0x41))
        .await
        .expect("签发");
    let command = auction_application::consignment::WithdrawDepositCommand {
        command_id: client_command(11),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(super::helpers::USER).expect("固定用户"),
        ),
        listing_id: listing_id(),
        expected_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
        request_hash: hash(0x42),
        request_id: None,
    };
    let receipt = repository.request_withdrawal(command.clone()).await.expect("关闭未代管件");
    assert_eq!(receipt.disposition.as_str(), "closed");
    assert_eq!(receipt.disposition.as_str(), WithdrawDisposition::Closed.as_str());
    assert_eq!(receipt.state, ListingState::Closed);
    let replay = repository.request_withdrawal(command).await.expect("重放");
    assert_eq!(replay, receipt);
    let listing: (String, Option<String>) =
        sqlx::query_as("SELECT state, reason_code FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id())
            .fetch_one(pool)
            .await
            .expect("读取挂单");
    assert_eq!(listing, ("closed".to_owned(), Some("seller_cancelled".to_owned())));
    let reservation: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id=$1",
    )
    .bind(issued.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(reservation, ("retired".to_owned(), Some("cancelled".to_owned())));
    let holder: Option<Uuid> = sqlx::query_scalar(
        "SELECT reservation_id FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder, None);
}

#[tokio::test]
async fn postgres_deposit_withdraw_intent_and_admin_hold_are_replayable() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    let repository = repository(&database).await;
    let withdraw = auction_application::consignment::WithdrawDepositCommand {
        command_id: client_command(20),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(USER_1).expect("固定卖家"),
        ),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(4).expect("版本"),
        request_hash: hash(0x51),
        request_id: None,
    };
    let intent = repository.request_withdrawal(withdraw.clone()).await.expect("登记意愿");
    assert_eq!(intent.state, ListingState::DepositAccepted);
    assert!(intent.intent_id.is_some());
    assert_eq!(repository.request_withdrawal(withdraw).await.expect("重放"), intent);
    let hold = auction_application::consignment::SetOperationsHoldCommand {
        admin_actor: "fake-admin".to_owned(),
        command_id: client_command(21),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(5).expect("版本"),
        request_hash: hash(0x52),
        request_id: None,
        reason: auction_application::consignment::ListingDelistReason::new(
            "fixture reason".to_owned(),
        )
        .expect("说明"),
    };
    let held = repository.set_operations_hold(hold.clone()).await.expect("设止售");
    assert_eq!(held.state, ListingState::HeldOffMarket);
    assert!(held.hold_id.is_some());
    assert_eq!(repository.set_operations_hold(hold).await.expect("重放"), held);
    let release = auction_application::consignment::ReleaseOperationsHoldCommand {
        admin_actor: "fake-admin".to_owned(),
        command_id: client_command(22),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(6).expect("版本"),
        hold_id: held.hold_id.expect("止售 ID"),
        request_hash: hash(0x53),
        request_id: None,
    };
    let released = repository.release_operations_hold(release.clone()).await.expect("解除止售");
    assert_eq!(released.state, ListingState::HeldOffMarket);
    assert_eq!(
        released.held_reason,
        Some(auction_domain::consignment::HeldOffMarketReason::Review)
    );
    assert_eq!(repository.release_operations_hold(release).await.expect("重放"), released);
    let controls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_controls
         WHERE position_id=(SELECT id FROM public.consignment_custody_positions WHERE listing_id=$1)",
    )
    .bind(Uuid::parse_str(support::LISTING).expect("固定挂单"))
    .fetch_one(pool)
    .await
    .expect("读取控制数");
    assert_eq!(controls, 3, "意愿、设止售与解除各一条");
}

#[tokio::test]
async fn postgres_deposit_admin_delist_managed_requires_matching_hold() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    let repository = repository(&database).await;
    let command = auction_application::consignment::SetOperationsHoldCommand {
        admin_actor: "fake-admin".to_owned(),
        command_id: client_command(30),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(4).expect("版本"),
        request_hash: hash(0x61),
        request_id: None,
        reason: auction_application::consignment::ListingDelistReason::new(
            "fixture reason".to_owned(),
        )
        .expect("说明"),
    };
    repository.set_operations_hold(command).await.expect("设止售");
    let error = repository
        .release_operations_hold(auction_application::consignment::ReleaseOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(31),
            listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
            expected_version: auction_domain::consignment::ListingVersion::new(5).expect("版本"),
            hold_id: Uuid::new_v4(),
            request_hash: hash(0x62),
            request_id: None,
        })
        .await
        .expect_err("错误止售 ID");
    assert!(
        matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::HoldMismatch)),
        "{error:?}"
    );
}

#[tokio::test]
async fn postgres_operations_hold_rejects_second_active_hold_without_new_fact() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    let repository = repository(&database).await;
    let reason = || ListingDelistReason::new("duplicate hold".to_owned()).expect("说明");
    repository
        .set_operations_hold(SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(205),
            listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
            expected_version: auction_domain::consignment::ListingVersion::new(4).expect("版本"),
            request_hash: hash(0xd5),
            request_id: None,
            reason: reason(),
        })
        .await
        .expect("首次设止售");
    let error = repository
        .set_operations_hold(SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(206),
            listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
            expected_version: auction_domain::consignment::ListingVersion::new(5).expect("版本"),
            request_hash: hash(0xd6),
            request_id: None,
            reason: reason(),
        })
        .await
        .expect_err("同一 position 不得重复设止售");
    assert!(matches!(error, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed)));
    let holds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_controls
         WHERE position_id=(SELECT id FROM public.consignment_custody_positions WHERE listing_id=$1)
           AND kind='hold_set'",
    )
    .bind(Uuid::parse_str(support::LISTING).expect("固定挂单"))
    .fetch_one(pool)
    .await
    .expect("读取止售控制");
    assert_eq!(holds, 1, "拒绝后不得新增 hold_set");
}

#[tokio::test]
async fn postgres_v1_withdraw_retires_reservation_and_secret() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let issued = repository
        .issue_reservation(issue_command(client_command(60), listing_id(), 2, 0x90))
        .await
        .expect("签发预约");
    let command = WithdrawListingCommand {
        request_hash: hash(0x91),
        target: SellerListingTarget {
            command_id: client_command(61),
            seller_user_id: auction_domain::UserId::from_uuid(
                Uuid::parse_str(super::helpers::USER).expect("固定用户"),
            ),
            listing_id: listing_id(),
            expected_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
        },
        request_id: None,
    };
    let closed = repository
        .store()
        .withdraw(command)
        .await
        .expect("v1 撤回")
        .into_result()
        .expect("v1 撤回成功");
    assert_eq!(closed.listing_id(), listing_id());
    let reservation: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id=$1",
    )
    .bind(issued.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(reservation, ("retired".to_owned(), Some("cancelled".to_owned())));
    let secret: (Option<Vec<u8>>, Option<Vec<u8>>, bool) = sqlx::query_as(
        "SELECT ciphertext, nonce, retired_at IS NOT NULL
         FROM public.consignment_reservation_secrets WHERE reservation_id=$1",
    )
    .bind(issued.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取密文");
    assert_eq!(secret, (None, None, true));
    let holder: Option<Uuid> = sqlx::query_scalar(
        "SELECT reservation_id FROM public.consignment_equipment_claims
         WHERE world_id='main' AND canonical_eid='42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用");
    assert_eq!(holder, None);
}

#[tokio::test]
async fn postgres_deposit_pending_intent_new_command_replays_without_new_event() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    let repository = repository(&database).await;
    let first = auction_application::consignment::WithdrawDepositCommand {
        command_id: client_command(63),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(USER_1).expect("固定卖家"),
        ),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(4).expect("版本"),
        request_hash: hash(0x94),
        request_id: None,
    };
    let first_receipt = repository.request_withdrawal(first).await.expect("首次意愿");
    let second = auction_application::consignment::WithdrawDepositCommand {
        command_id: client_command(64),
        seller_user_id: auction_domain::UserId::from_uuid(
            Uuid::parse_str(USER_1).expect("固定卖家"),
        ),
        listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
        expected_version: auction_domain::consignment::ListingVersion::new(5).expect("版本"),
        request_hash: hash(0x95),
        request_id: None,
    };
    let second_receipt = repository.request_withdrawal(second).await.expect("重复意愿");
    assert_eq!(second_receipt.state, ListingState::DepositAccepted);
    assert_eq!(second_receipt.listing_version, first_receipt.listing_version);
    assert_eq!(second_receipt.intent_id, first_receipt.intent_id);
    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT state_version FROM public.consignment_events
         WHERE listing_id=$1 ORDER BY state_version",
    )
    .bind(Uuid::parse_str(support::LISTING).expect("固定挂单"))
    .fetch_all(pool)
    .await
    .expect("读取事件版本");
    assert_eq!(versions, vec![1, 2, 3, 4, 5], "重复意愿不得新增挂单版本");
}

#[tokio::test]
async fn postgres_v1_admin_delist_retires_reservation() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let repository = repository(&database).await;
    let issued = repository
        .issue_reservation(issue_command(client_command(65), listing_id(), 2, 0x96))
        .await
        .expect("签发预约");
    repository
        .store()
        .admin_delist(AdminDelistListingCommand {
            command_id: client_command(66),
            listing_id: listing_id(),
            expected_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
            admin_actor: "fake-admin".to_owned(),
            request_hash: hash(0x97),
            request_id: None,
            reason: ListingDelistReason::new("admin test".to_owned()).expect("说明"),
        })
        .await
        .expect("v1 Admin 下架")
        .into_result()
        .expect("v1 Admin 下架成功");
    let reservation: (String, Option<String>) = sqlx::query_as(
        "SELECT state, reason FROM public.consignment_deposit_reservations WHERE id=$1",
    )
    .bind(issued.reservation_id)
    .fetch_one(pool)
    .await
    .expect("读取预约");
    assert_eq!(reservation, ("retired".to_owned(), Some("cancelled".to_owned())));
}

#[tokio::test]
async fn postgres_withdraw_held_keeps_operations_reason() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("运行账号");
    let pool = runtime.postgres_pool();
    support::seed_accepted(pool).await;
    let repository = repository(&database).await;
    let held = repository
        .set_operations_hold(SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(67),
            listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
            expected_version: auction_domain::consignment::ListingVersion::new(4).expect("版本"),
            request_hash: hash(0x98),
            request_id: None,
            reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
        })
        .await
        .expect("设止售");
    let withdrawn = repository
        .request_withdrawal(auction_application::consignment::WithdrawDepositCommand {
            command_id: client_command(68),
            seller_user_id: auction_domain::UserId::from_uuid(
                Uuid::parse_str(USER_1).expect("固定卖家"),
            ),
            listing_id: Uuid::parse_str(support::LISTING).expect("固定挂单"),
            expected_version: auction_domain::consignment::ListingVersion::new(5).expect("版本"),
            request_hash: hash(0x99),
            request_id: None,
        })
        .await
        .expect("止售件登记意愿");
    assert_eq!(withdrawn.state, ListingState::HeldOffMarket);
    assert!(withdrawn.intent_id.is_some());
    assert_ne!(withdrawn.intent_id, held.hold_id);
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason_code FROM public.consignment_listings WHERE id=$1")
            .bind(Uuid::parse_str(support::LISTING).expect("固定挂单"))
            .fetch_one(pool)
            .await
            .expect("读取挂单原因");
    assert_eq!(reason.as_deref(), Some("operations_hold"));
}

/// 停用且无预约码密码的装配（D9 §6.3、§7）：签发预约失败关闭且零写入，撤回照常。
#[tokio::test]
async fn postgres_deposit_without_reservation_crypto_still_withdraws_but_cannot_issue() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    seed_mail_ready(pool).await;
    let settings = auction_storage::DepositSettings::new(
        auction_domain::consignment::FeeBps::new(500).expect("测试费率"),
        auction_domain::consignment::ListingPrice::new(
            auction_domain::consignment::ListingPrice::MIN,
        )
        .expect("测试下限"),
        auction_domain::consignment::ListingPrice::new(
            auction_domain::consignment::ListingPrice::MAX,
        )
        .expect("测试上限"),
        24,
        100,
    );
    let repository = auction_storage::DepositRepository::without_reservation_crypto(
        database.store().await.expect("运行账号"),
        settings,
    );
    let issue = repository
        .issue_reservation(issue_command(client_command(20), listing_id(), 2, 0x51))
        .await
        .expect_err("无密码实现时签发必须失败关闭");
    assert!(matches!(issue, ConsignmentError::Unavailable));
    let reservations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_deposit_reservations")
            .fetch_one(pool)
            .await
            .expect("预约数");
    assert_eq!(reservations, 0, "失败的签发不得落预约");
    let receipt = repository
        .request_withdrawal(auction_application::consignment::WithdrawDepositCommand {
            command_id: client_command(21),
            seller_user_id: auction_domain::UserId::from_uuid(
                Uuid::parse_str(super::helpers::USER).expect("固定用户"),
            ),
            listing_id: listing_id(),
            expected_version: auction_domain::consignment::ListingVersion::new(2).expect("版本"),
            request_hash: hash(0x52),
            request_id: None,
        })
        .await
        .expect("无密码实现时撤回照常");
    assert_eq!(receipt.state, ListingState::Closed);
}
