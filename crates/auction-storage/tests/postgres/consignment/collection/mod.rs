//! 收款与确领推进循环（PIWQ）的核心规则：只读观察到领取后，经既有写口恰一次记录证据、认证实收、确领原件并
//! 放行卖款；验证器未签收时零读取、零推进（写口另有一道签收复核）；重放与重启零新增；全程零 POST、零 Take。
//! 两世界端到端（卖款与退款到受益人确领）见 `acceptance/two_worlds.rs`。
//!
//! 保护不变量 1（重放返回原结果）、4（只读远端、零外写）、6（未签收失败关闭）。

mod fixture;
mod paging;
mod replay;

use auction_application::settlement::FundingError;
use auction_jobs::consignment::DispatchRound;

use self::fixture::{Collector, UNSIGNED_COLLECTION_VALIDATOR};
use super::acceptance::chain::{Order, World};
use super::returns::fixture::UNSIGNED_VALIDATOR;

/// 本合同某类证据的条数。
async fn evidence(order: &Order, category: &str) -> i64 {
    order
        .scalar(format!(
            "SELECT count(*) FROM public.consignment_dispatch_evidence WHERE category = '{category}'"
        ))
        .await
}

#[tokio::test]
async fn postgres_consignment_collection_main_cod_certifies_hands_off_and_releases_payout_once() {
    let order = Order::available(World::Main).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    assert_eq!(order.posts(), 1);
    let takes = order.take_write_starts().await;

    // 未领取：读一次，零证据、零认证。
    let collector = Collector::signed();
    collector.set_collected(false);
    let waiting = order.collect_with(&collector).await;
    assert_eq!((waiting.advanced, waiting.failed, collector.reads()), (0, false, 1));
    assert_eq!(evidence(&order, "original_claim").await, 0);
    assert_eq!(order.count("consignment_cod_receipts").await, 0);

    // 已领取：一页内恰记一条证据、认证一次、确领一次并放行卖款。
    collector.set_collected(true);
    let round = order.collect_with(&collector).await;
    assert_eq!((round.advanced, round.failed), (3, false));
    assert_eq!(evidence(&order, "original_claim").await, 1);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.count("consignment_funded_sales").await, 1);
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);
    assert_eq!(order.business_state("seller_payout").await, "ready");
    assert_eq!(order.position_state().await, "handed_over");

    // 重启（新运行时）与重放：已收口的轮不再待办，零读取、零新增；全程零 POST、零 Take。
    let entries = order.count("settlement_funding_entries").await;
    let restarted = Collector::signed();
    assert_eq!(order.collect_with(&restarted).await.advanced, 0);
    assert_eq!(restarted.reads(), 0);
    assert_eq!(order.count("settlement_funding_entries").await, entries);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);
    assert_eq!(order.posts(), 1);
    assert_eq!(order.take_write_starts().await, takes, "零新增 Take");
}

#[tokio::test]
async fn postgres_consignment_collection_unsigned_validator_reads_and_advances_nothing() {
    let order = Order::available(World::Main).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);

    // 生产观察端口的未签收版本：零读取、零证据、零认证、零确领，卖款仍 blocked。
    let collector = Collector::with_version(UNSIGNED_COLLECTION_VALIDATOR);
    assert_eq!(order.collect_with(&collector).await.advanced, 0);
    assert_eq!(collector.reads(), 0);
    assert_eq!(evidence(&order, "original_claim").await, 0);
    assert_eq!(order.count("consignment_cod_receipts").await, 0);
    assert_eq!(order.count("consignment_handoff_proofs").await, 0);
    assert_eq!(order.business_state("seller_payout").await, "blocked");

    // 第二道：即使未签收的付款观察已被直接记成证据，认证写口也按缺证整笔回滚，零收据、零分录。
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.record_original_claim_by(attempt, "fake-cod-unsigned", UNSIGNED_VALIDATOR).await;
    let entries = order.count("settlement_funding_entries").await;
    assert!(matches!(order.certify(attempt).await, Err(FundingError::Unproven)));
    assert_eq!(order.count("consignment_cod_receipts").await, 0);
    assert_eq!(order.count("settlement_funding_entries").await, entries);
    assert_eq!(order.posts(), 1);
}

#[tokio::test]
async fn postgres_consignment_collection_isekai_carrier_payment_and_handoff_advance_separately() {
    let order = Order::available(World::Isekai).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    let takes = order.take_write_starts().await;

    // 载体付款：证据与认证各一次，同事务放行原件交割；卖款仍等确领。
    let paid = order.run_collections().await;
    assert_eq!((paid.advanced, paid.failed), (2, false));
    assert_eq!(evidence(&order, "carrier_payment").await, 1);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.business_state("winner_equipment_handoff").await, "ready");
    assert_eq!(order.business_state("seller_payout").await, "blocked");
    assert_eq!(order.count("consignment_handoff_proofs").await, 0);
    assert_eq!(order.run_collections().await.advanced, 0, "已认证的载体轮不再推进");

    // 交割轮在 isekai 派发后被领取：证据与确领各一次，放行卖款。
    assert_eq!(order.produce_rounds().await, 1);
    order.send_from("isekai").await;
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    order.send_from("main").await;
    let handed = order.run_collections().await;
    assert_eq!((handed.advanced, handed.failed), (2, false));
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);
    assert_eq!(order.business_state("seller_payout").await, "ready");
    assert_eq!(order.run_collections().await.advanced, 0);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.posts(), 2, "推进循环零 POST");
    assert_eq!(order.take_write_starts().await, takes, "零新增 Take");
}
