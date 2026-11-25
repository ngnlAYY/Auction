//! 并发与提交顺序：两买家恰一个成功；改价、止售与购买交换提交顺序，结果只有一种一致的终局。
//!
//! 保护不变量 2（同事务锁定与重验，禁止先读价后写入）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, CustodyControlStore, DepositBusinessRejection, ListingDelistReason,
    PurchaseOutcome, PurchaseStore, RepriceV2Outcome, RepriceV2Store, SetOperationsHoldCommand,
};
use auction_domain::consignment::purchase::{PurchaseRejection, RepriceRejection};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingVersion};
use auction_storage::{DepositRepository, DepositSettings, TestAfterLock};
use tokio::sync::Notify;
use tokio::time::sleep;

use super::fixture::{
    BUYER, Fixture, PRICE, USER_3, VERSION, command_id, digest, listing_id, purchase, purchase_at,
    reprice,
};

/// 取锁后停住、直到 `release` 被通知才继续开快照的钩子；`entered` 在停住时通知。
fn pause_hook(entered: Arc<Notify>, release: Arc<Notify>) -> Box<TestAfterLock> {
    Box::new(move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (entered, release) = (entered.clone(), release.clone());
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
        })
    })
}

/// 两买家并发购买同一挂单：恰一个成交，另一个得 `listing_not_active`，只有一份合同与首轮。
#[tokio::test]
async fn postgres_consignment_purchase_two_buyers_exactly_one_wins() {
    let fixture = Fixture::available().await;
    let (first, second) = tokio::join!(
        fixture.repository.purchase(purchase(BUYER, 1, 0x11)),
        fixture.repository.purchase(purchase(USER_3, 2, 0x12)),
    );
    let outcomes = [first.expect("第一买家"), second.expect("第二买家")];
    let wins =
        outcomes.iter().filter(|outcome| matches!(outcome, PurchaseOutcome::Purchased(_))).count();
    let losses = outcomes
        .iter()
        .filter(|outcome| {
            matches!(outcome, PurchaseOutcome::Rejected(PurchaseRejection::ListingNotActive))
        })
        .count();
    assert_eq!((wins, losses), (1, 1), "{outcomes:?}");
    assert_eq!(fixture.count("consignment_contracts").await, 1);
    assert_eq!(fixture.count("consignment_sales").await, 1);
    assert_eq!(fixture.count("consignment_dispatch_rounds").await, 1);
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
}

/// 购买先取到锁：改价必须等它提交，之后得确定拒绝，Sale 冻结原价。
/// 改价先提交：带旧版本的购买得 `stale_version`，无任何成交事实；按新版本与新价再买，Sale 冻结新价。
#[tokio::test]
async fn postgres_consignment_purchase_and_reprice_swap_commit_order() {
    // 顺序一：购买先持锁。
    let fixture = Fixture::available().await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let buying = {
        let (repository, entered, release) =
            (fixture.repository.clone(), entered.clone(), release.clone());
        tokio::spawn(async move {
            let hook = pause_hook(entered, release);
            repository.test_purchase_hooked(purchase(BUYER, 1, 0x11), Some(&*hook)).await
        })
    };
    entered.notified().await;
    let repricing = {
        let repository = fixture.repository.clone();
        tokio::spawn(async move { repository.reprice(reprice(2, 0x22, VERSION, 150)).await })
    };
    sleep(Duration::from_millis(250)).await;
    assert!(!repricing.is_finished(), "改价应被购买持有的卖家锁挡住");
    release.notify_one();
    let bought = buying.await.expect("购买任务").expect("购买");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let repriced = repricing.await.expect("改价任务").expect("改价");
    assert!(
        matches!(
            repriced,
            RepriceV2Outcome::Rejected(
                RepriceRejection::EquipmentCommitted | RepriceRejection::ListingNotActive
            )
        ),
        "{repriced:?}"
    );
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
    let sale_price: i64 = sqlx::query_scalar("SELECT price FROM public.consignment_sales")
        .fetch_one(fixture.pool())
        .await
        .expect("Sale 价");
    assert_eq!(sale_price, PRICE, "已成交合同的价格不随改价变化");

    // 顺序二：改价先提交。
    let fixture = Fixture::available().await;
    let repriced = fixture.repository.reprice(reprice(2, 0x22, VERSION, 150)).await.expect("改价");
    assert!(matches!(repriced, RepriceV2Outcome::Accepted(_)), "{repriced:?}");
    let stale = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("旧版本购买");
    assert!(
        matches!(stale, PurchaseOutcome::Rejected(PurchaseRejection::StaleVersion)),
        "{stale:?}"
    );
    assert_eq!(fixture.count("consignment_contracts").await, 0);
    assert_eq!(fixture.position_state().await, "in_custody");
    let fresh = fixture
        .repository
        .purchase(purchase_at(BUYER, 3, 0x13, VERSION + 1, 150))
        .await
        .expect("新价购买");
    assert!(matches!(fresh, PurchaseOutcome::Purchased(_)), "{fresh:?}");
    let (price, fee, seller_amount): (i64, i64, i64) =
        sqlx::query_as("SELECT price, fee, seller_amount FROM public.consignment_sales")
            .fetch_one(fixture.pool())
            .await
            .expect("Sale");
    assert_eq!((price, fee, seller_amount), (150, 7, 143));
}

/// 运营止售与购买交换提交顺序：止售先提交则购买得确定拒绝且无成交事实；购买先提交则止售失败，
/// 挂单保持 sale_pending。
#[tokio::test]
async fn postgres_consignment_purchase_and_operations_hold_swap_commit_order() {
    let hold_command = |seed: u128, version: u64| SetOperationsHoldCommand {
        admin_actor: "fake-admin".to_owned(),
        command_id: command_id(seed),
        listing_id: listing_id(),
        expected_version: ListingVersion::new(version).expect("版本"),
        request_hash: digest(0x91),
        request_id: None,
        reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
    };
    let deposit = |fixture: &Fixture| {
        DepositRepository::without_reservation_crypto(
            fixture.runtime.clone(),
            DepositSettings::new(
                FeeBps::new(500).expect("费率"),
                ListingPrice::new(ListingPrice::MIN).expect("下限"),
                ListingPrice::new(ListingPrice::MAX).expect("上限"),
                24,
                100,
            ),
        )
    };

    // 止售先提交。
    let fixture = Fixture::available().await;
    deposit(&fixture).set_operations_hold(hold_command(2, VERSION)).await.expect("止售");
    let blocked = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    assert!(
        matches!(blocked, PurchaseOutcome::Rejected(PurchaseRejection::ListingNotActive)),
        "{blocked:?}"
    );
    assert_eq!(fixture.count("consignment_contracts").await, 0);
    assert_eq!(fixture.count("consignment_sales").await, 0);
    assert_eq!(fixture.count("settlement_obligations").await, 0);
    assert_eq!(fixture.position_state().await, "in_custody");
    assert_eq!(fixture.listing().await.0, "held_off_market");

    // 购买先提交。
    let fixture = Fixture::available().await;
    let bought = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let held = deposit(&fixture).set_operations_hold(hold_command(2, VERSION + 1)).await;
    assert!(
        matches!(held, Err(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))),
        "已成交的挂单不能再被止售：{held:?}"
    );
    // 被拒的止售零副作用：合同、Sale 与位置原样，挂单没有任何止售事件。
    assert_eq!(fixture.count("consignment_contracts").await, 1);
    assert_eq!(fixture.count("consignment_sales").await, 1);
    assert_eq!(fixture.position_state().await, "committed");
    let hold_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_events WHERE to_state = 'held_off_market'",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("止售事件");
    assert_eq!(hold_events, 0);
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
}
