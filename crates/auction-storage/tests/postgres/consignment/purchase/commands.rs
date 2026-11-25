//! 购买命令：一次提交全部事实、同 ID 重放得原回执、同 ID 异请求 / 异演员 / 异操作冲突。
//!
//! 保护不变量 1（命令幂等）与 3（接受的事实、版本、事件与回执同事务）。

use auction_application::consignment::{
    ConsignmentError, PurchaseOutcome, PurchaseStore, RepriceV2Store,
};
use auction_domain::consignment::OrderState;

use super::fixture::{BUYER, Fixture, PRICE, USER_3, purchase, purchase_at, reprice};

/// 购买成功一次提交 Sale、义务图、首轮、sale_pending、committed、事件与回执；同 ID 重放得原结果，
/// 不增任何行。
#[tokio::test]
async fn postgres_consignment_purchase_commits_every_fact_once_and_replays_the_original() {
    let fixture = Fixture::available().await;
    let outcome = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(receipt) = outcome else {
        panic!("应成交：{outcome:?}");
    };
    assert!(!receipt.replayed);
    assert_eq!(receipt.listing_version.value(), 6);
    assert_eq!(receipt.contract_version, 1);
    assert_eq!(receipt.order_state, OrderState::AwaitingPayment);
    assert_eq!(receipt.price.value(), PRICE);

    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
    assert_eq!(fixture.position_state().await, "committed");
    // main Sale 的义务图：COD 原件、卖款、退款三条，各一条履约行；首轮的投递停在 prepared。
    assert_eq!(fixture.count("consignment_contracts").await, 1);
    assert_eq!(fixture.count("consignment_sales").await, 1);
    assert_eq!(fixture.count("settlement_obligations").await, 3);
    assert_eq!(fixture.count("settlement_obligation_fulfillments").await, 3);
    assert_eq!(fixture.count("consignment_dispatch_rounds").await, 1);
    let prepared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'settlement' AND status = 'prepared'",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("首轮投递");
    assert_eq!(prepared, 1);
    // 金额与 D0 只由冻结价格、费率与策略推出：P=101、bps=500 得 F=5、S=96；D0 = 成交时刻 + 7 天。
    let (fee, seller_amount, days): (i64, i64, i64) = sqlx::query_as(
        "SELECT fee, seller_amount,
                (extract(epoch FROM sale.payment_deadline_at - contract.created_at) / 86400)::bigint
         FROM public.consignment_sales sale
         JOIN public.consignment_contracts contract USING (contract_id)",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("Sale");
    assert_eq!((fee, seller_amount, days), (5, 96, 7));
    let purchased_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_events WHERE event_kind = 'purchased' AND state_version = 6",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("事件");
    assert_eq!(purchased_events, 1);

    // 同 ID 同请求重放：原 201 回执，零新增事实。
    let replay = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("重放");
    let PurchaseOutcome::Purchased(again) = replay else {
        panic!("重放应回原回执：{replay:?}");
    };
    assert!(again.replayed);
    assert_eq!(
        (again.contract_id, again.listing_version),
        (receipt.contract_id, receipt.listing_version)
    );
    assert_eq!(fixture.count("consignment_contracts").await, 1);
    assert_eq!(fixture.count("settlement_obligations").await, 3);
    assert_eq!(fixture.count("consignment_dispatch_rounds").await, 1);
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
}

/// 同命令 ID 的异请求、异演员、异操作一律冲突，且不写任何新事实。
#[tokio::test]
async fn postgres_consignment_purchase_command_id_conflicts_across_request_actor_and_operation() {
    let fixture = Fixture::available().await;
    fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let commands_before = fixture.count("consignment_commands").await;

    // 异请求：摘要不同。
    let changed = fixture.repository.purchase(purchase(BUYER, 1, 0x12)).await;
    assert!(matches!(changed, Err(ConsignmentError::CommandIdConflict)), "{changed:?}");
    // 异演员：另一个买家复用同一命令 ID。
    let other_actor = fixture.repository.purchase(purchase(USER_3, 1, 0x11)).await;
    assert!(matches!(other_actor, Err(ConsignmentError::CommandIdConflict)), "{other_actor:?}");
    // 异操作：同一命令 ID 用于改价。
    let other_operation = fixture.repository.reprice(reprice(1, 0x11, 6, 150)).await;
    assert!(
        matches!(other_operation, Err(ConsignmentError::CommandIdConflict)),
        "{other_operation:?}"
    );
    // 异目标：同买家同摘要但不同预期版本不是同一请求（摘要由应用层按全部输入计算，这里换摘要模拟）。
    let other_input = fixture.repository.purchase(purchase_at(BUYER, 1, 0x13, 5, PRICE)).await;
    assert!(matches!(other_input, Err(ConsignmentError::CommandIdConflict)), "{other_input:?}");

    assert_eq!(fixture.count("consignment_commands").await, commands_before);
    assert_eq!(fixture.count("consignment_contracts").await, 1);
    // 没被占用的命令 ID 不受影响，且只能得到确定拒绝（挂单已 sale_pending）。
    let rejected = fixture.repository.purchase(purchase(USER_3, 2, 0x21)).await.expect("确定拒绝");
    assert!(matches!(rejected, PurchaseOutcome::Rejected(_)), "{rejected:?}");
}
