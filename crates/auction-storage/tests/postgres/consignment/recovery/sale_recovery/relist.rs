//! 重列：Sale 已封闭、原件在库的止售件手动重列为 g+1 `relist` 代；之后第二位买家可购买，旧 Sale 的外键
//! 与封闭事实仍可查，不产生虚构的订单或资金分录。未封闭的 Sale 不能重列。
//!
//! 本人详情的重列入口（`relist_allowed`）与写端同一判据：可重列时宣告且写端接受，不可重列时不宣告且写端拒绝。
//!
//! 保护的不变量：1（命令幂等）、2（唯一后继与购买竞争由 (position, 代) 兜底）、3（代、事件、回执同提）、6。

use auction_application::consignment::{
    ConsignmentError, CustodyRelistStore, DepositBusinessRejection, OwnedListingReadV2,
    PurchaseCommand, PurchaseOutcome, PurchaseStore, RelistCommand,
};
use auction_application::core::RequestHash;
use auction_domain::UserId;
use auction_domain::consignment::deposit::SellerAction;
use auction_domain::consignment::purchase::PurchaseInput;
use auction_domain::consignment::{ListingPrice, ListingState, ListingVersion};
use serde_json::json;

use super::super::super::contracts::fixture::CONTRACT;
use super::super::super::deposit::support::LISTING;
use super::super::super::deposit::support::succeeds;
use super::super::super::ledger136::fixture::original_dispatch_sql;
use super::super::command_sql::{command, event, listing_update};
use super::{
    BUYER_3, Closure, PRICE, World, closure_sql, command_id, database_time, id, seller,
    unknown_refund_sql,
};

/// 夹具解除止售命令（明显的假 UUIDv4）。
const RELEASE: &str = "c7f30000-0000-4000-8000-000000000001";

fn relist(seed: u128, version: u64, hash: u8) -> RelistCommand {
    RelistCommand {
        command_id: command_id(seed),
        seller_user_id: seller(),
        listing_id: id(LISTING),
        expected_version: ListingVersion::new(version).expect("版本"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

fn purchase(seed: u128, version: u64) -> PurchaseCommand {
    PurchaseCommand {
        command_id: command_id(seed),
        request_hash: RequestHash::new([0x41; 32]),
        buyer_user_id: UserId::from_uuid(id(BUYER_3)),
        input: PurchaseInput {
            listing_id: id(LISTING),
            expected_version: ListingVersion::new(version).expect("版本"),
            expected_price: ListingPrice::new(PRICE).expect("价格"),
        },
        request_id: None,
    }
}

/// 正例：重列后挂单回到 `available`、position 回库且指针在 g+1；第二位买家在新代上购买成功，
/// 新 Sale 占代 2；旧 Sale 与封闭事实仍可经外键查到；没有任何资金分录。同命令重放返回原回执。
#[tokio::test]
async fn postgres_relist_after_a_sealed_sale_lets_a_second_buyer_purchase() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let command = relist(1, 6, 0x51);

    let receipt = world.deposit.relist(command.clone()).await.expect("重列");

    assert_eq!(
        (receipt.state, receipt.listing_version.value(), receipt.generation),
        (ListingState::Available, 7, 2)
    );
    assert_eq!(world.listing().await, ("available".to_owned(), 7));
    assert_eq!(world.position().await, ("in_custody".to_owned(), Some(2), 5));
    assert_eq!(world.generations().await.last().cloned(), Some((2, "relist".to_owned(), Some(1))));
    assert_eq!(world.listing_events("relisted").await, 1);
    let rows = world.count("consignment_commands").await;
    // 只读重放（软关闭路径）回原回执，未知命令为空，两者都不写账本。
    assert_eq!(
        world.deposit.replay_relist(command.clone()).await.expect("只读重放"),
        Some(receipt.clone())
    );
    assert_eq!(world.deposit.replay_relist(relist(9, 6, 0x51)).await.expect("未知命令"), None);
    assert_eq!(world.deposit.relist(command).await.expect("同命令重放"), receipt);
    assert_eq!(world.count("consignment_commands").await, rows);

    let bought = world.purchases.purchase(purchase(2, 7)).await.expect("第二次购买");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");

    let (old_buyer, new_buyer): (String, String) = sqlx::query_as(
        "SELECT (SELECT old_sale.buyer_user_id::text
                 FROM public.consignment_contracts old_contract
                 JOIN public.consignment_sales old_sale ON old_sale.contract_id = old_contract.contract_id
                 JOIN public.consignment_sale_closures closure
                   ON closure.contract_id = old_contract.contract_id
                 WHERE old_contract.generation = 1),
                (SELECT new_sale.buyer_user_id::text
                 FROM public.consignment_contracts new_contract
                 JOIN public.consignment_sales new_sale ON new_sale.contract_id = new_contract.contract_id
                 WHERE new_contract.generation = 2)",
    )
    .fetch_one(&world.pool)
    .await
    .expect("两代 Sale");
    assert_ne!(old_buyer, new_buyer);
    assert_eq!(new_buyer, BUYER_3);
    assert_eq!(world.position().await.0, "committed");
    assert_eq!(world.count("consignment_sales").await, 2);
    assert_eq!(world.count("settlement_funding_entries").await, 0, "没有虚构的资金分录");
    let old_still: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_contracts WHERE contract_id = $1)",
    )
    .bind(id(CONTRACT))
    .fetch_one(&world.pool)
    .await
    .expect("旧合同");
    assert!(old_still);
}

/// 反例：Sale 未封闭时不能重列（没有“可再售”的处置依据）；没有追加处分代，挂单与 position 不变。
#[tokio::test]
async fn postgres_relist_refuses_when_the_sale_is_not_sealed() {
    let world = World::sold(Closure::None).await;
    let before = (world.position().await, world.listing().await);

    let refused = world.deposit.relist(relist(3, 6, 0x52)).await;

    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))
        ),
        "{refused:?}"
    );
    assert_eq!((world.position().await, world.listing().await), before);
    assert_eq!(world.generations().await.len(), 1);
    assert_eq!(world.listing_events("relisted").await, 0);
}

/// 反例：Sale 已封闭，但（a）原件腿已发出而未精确收回；（b）同合同买家退款派发结果未知。
/// 两种情形重列都被拒，没有追加处分代，挂单与 position 不变。
#[tokio::test]
async fn postgres_relist_refuses_an_unreturned_original_and_an_unknown_refund_dispatch() {
    let sent = World::sold(Closure::None).await;
    succeeds(&sent.owner, &original_dispatch_sql()).await;
    succeeds(&sent.owner, &closure_sql("cancelled_unpaid")).await;
    let before = (sent.position().await, sent.listing().await);
    let refused = sent.deposit.relist(relist(4, 6, 0x53)).await;
    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))
        ),
        "{refused:?}"
    );
    assert_eq!((sent.position().await, sent.listing().await), before);
    assert_eq!(sent.generations().await.len(), 1);

    let unknown = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&unknown.owner, &unknown_refund_sql()).await;
    let before = (unknown.position().await, unknown.listing().await);
    let refused = unknown.deposit.relist(relist(5, 6, 0x54)).await;
    assert!(
        matches!(refused, Err(ConsignmentError::Deposit(DepositBusinessRejection::EquipmentBusy))),
        "{refused:?}"
    );
    assert_eq!((unknown.position().await, unknown.listing().await), before);
    assert_eq!(unknown.generations().await.len(), 1);
    assert_eq!(unknown.listing_events("relisted").await, 0);
}

/// 夹具的 `operations_hold` 没有有效止售控制行，v2 本人读按形状失败关闭；本人详情的用例先经一条
/// Admin 解除止售把原因推进为 `review`（版本 7），每条语句仍过同一套挂单与事件守卫。
async fn released_for_review(world: &World) {
    let at = database_time(&world.owner).await;
    let body = json!({"schema_version": 2, "operation": "release_hold",
        "disposition": "hold_released", "command_id": RELEASE, "listing_id": LISTING,
        "listing_version": 7, "state": "held_off_market", "held_reason": "review"});
    let sql = command(
        RELEASE,
        ("admin", "fake-admin"),
        "release_hold",
        ("listing", Some(LISTING), LISTING),
        Some(LISTING),
        200,
        &body,
        &at,
    ) + &event(
        7,
        RELEASE,
        ("admin", "hold_released"),
        ("held_off_market", "held_off_market"),
        Some("review"),
        &at,
    ) + &listing_update("held_off_market", Some("review"), 7, &at);
    succeeds(&world.owner, &sql).await;
}

/// 本人详情此刻是否宣告重列（经与 Public 装配相同的 `DepositRepository` 读口）。
async fn offers_relist(world: &World) -> bool {
    world
        .deposit
        .owned_detail_v2(seller(), id(LISTING))
        .await
        .expect("本人详情")
        .expect("本人挂单")
        .seller_actions()
        .contains(&SellerAction::Relist)
}

/// 正例：Sale 已按未付取消封闭、原件在库时，本人详情宣告重列，写端随后接受；重列上架后不再宣告。
#[tokio::test]
async fn postgres_owned_detail_offers_relist_when_the_write_side_accepts() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    released_for_review(&world).await;

    assert!(offers_relist(&world).await);
    let receipt = world.deposit.relist(relist(6, 7, 0x55)).await.expect("重列");

    assert_eq!(receipt.state, ListingState::Available);
    assert!(!offers_relist(&world).await);
}

/// 反例：Sale 已封闭但原件腿已发出而未精确收回时，本人详情不宣告重列；写端同样拒绝，不追加处分代。
#[tokio::test]
async fn postgres_owned_detail_hides_relist_when_the_write_side_refuses() {
    let sent = World::sold(Closure::None).await;
    succeeds(&sent.owner, &original_dispatch_sql()).await;
    succeeds(&sent.owner, &closure_sql("cancelled_unpaid")).await;
    released_for_review(&sent).await;

    assert!(!offers_relist(&sent).await);
    let refused = sent.deposit.relist(relist(7, 7, 0x56)).await;

    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))
        ),
        "{refused:?}"
    );
    assert_eq!(sent.generations().await.len(), 1);
}
