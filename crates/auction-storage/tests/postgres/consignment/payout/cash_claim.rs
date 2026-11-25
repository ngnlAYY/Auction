//! 受益人现金确领（D9 §2.3 `remote_cash_claimed`、§6.2 `settle_cash` 的 claimed 必引远端确领）：只有已签收
//! 验证器的现金确领证据才把已支出的 S 记为 claimed，恰一次。
//!
//! 保护不变量 1、4、6：未签收的观察不成为结论；没有确领证据的 claimed 直接推进失败关闭；重放零新增。

use auction_application::settlement::{CashLeg, FundingError};
use auction_jobs::consignment::DispatchRound;

use super::super::acceptance::chain::{Order, World};
use super::super::returns::fixture::{SIGNED_VALIDATOR, UNSIGNED_VALIDATOR};

#[tokio::test]
async fn postgres_consignment_cash_claim_requires_signed_evidence_and_claims_once() {
    let order = Order::available(World::Main).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    let attempt = order.first_round(order.obligation("winner_equipment_cod").await).await;
    order.record_payment(attempt).await;
    order.certify(attempt).await.expect("实收认证");
    order.prove_handoff("winner_equipment_cod", "fake-cod-paid-1", [0xe1; 32]).await.expect("确领");
    assert_eq!(order.produce_rounds().await, 1);
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    let payout = order.first_round(order.obligation("seller_payout").await).await;
    let seller_amount: i64 =
        order.scalar("SELECT seller_amount FROM public.consignment_sales".to_owned()).await;
    assert_eq!(order.allocation("seller").await, Some(("spent".to_owned(), seller_amount)));

    // 反例：没有确领证据，直接推进 claimed 未证明；未签收验证器的观察只记证据、不记 claimed。
    let direct = order.settle_claimed("seller_payout", CashLeg::Seller, payout, seller_amount);
    assert!(matches!(direct.await, Err(FundingError::Unproven)));
    assert!(
        !order.cash_claim(payout, "fake-cash-claim-1", UNSIGNED_VALIDATOR).await.expect("观察")
    );
    assert_eq!(order.entries("claimed").await, (0, 0));
    let direct = order.settle_claimed("seller_payout", CashLeg::Seller, payout, seller_amount);
    assert!(matches!(direct.await, Err(FundingError::Unproven)));

    // 正例：已签收的现金确领证据同事务恰记一条 claimed；重放与再推进都零新增。
    assert!(!order.cash_claim(payout, "fake-cash-claim-2", SIGNED_VALIDATOR).await.expect("确领"));
    assert_eq!(order.entries("claimed").await, (1, seller_amount));
    let entries = order.count("settlement_funding_entries").await;
    assert!(order.cash_claim(payout, "fake-cash-claim-2", SIGNED_VALIDATOR).await.expect("重放"));
    let direct = order.settle_claimed("seller_payout", CashLeg::Seller, payout, seller_amount);
    assert!(direct.await.expect("已确领只登记命令"));
    assert_eq!(order.count("settlement_funding_entries").await, entries);
    assert_eq!(order.entries("claimed").await, (1, seller_amount));
}
