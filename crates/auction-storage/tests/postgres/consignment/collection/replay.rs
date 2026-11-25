//! 收款认证命令的幂等与并发（PIWQ）：同一原付款义务、同一原投递，原输入重放读原结果，换所消费的付款证据
//! （摘要或远端观察身份）即异请求冲突；两个推进循环并发处理同一轮，证据、收据与确领各恰一份。
//!
//! 保护不变量 1（同 ID 同请求返回原结果，同 ID 异请求冲突）、2（并发只产生一份经济结果）。

use auction_application::consignment::{CollectedEvidence, certify_cod_command};
use auction_application::settlement::{ConsignmentFundingStore, FundingError};
use auction_jobs::consignment::DispatchRound;
use auction_storage::CollectedRoundRepository;

use super::super::acceptance::chain::{Order, World};
use super::super::dispatch::carrier_funding::BALANCE;
use super::super::funding::cod_fixture::cash;
use super::fixture::{Collector, collected_digest};

/// main 订单首派后，推进循环跑一次扫描（远端已领取）。
async fn dispatched_main() -> Order {
    let order = Order::available(World::Main).await;
    order.purchase().await.expect("购买");
    assert_eq!(order.run_executor().await, DispatchRound::Sent);
    order
}

#[tokio::test]
async fn postgres_consignment_collection_certify_replays_and_rejects_swapped_evidence() {
    let order = dispatched_main().await;
    // 认证前从生产来源取冻结事实，之后由推进循环认证一次（生产 encoder）。
    let page =
        CollectedRoundRepository::new(order.runtime.clone()).pending(None, 32).await.expect("待办");
    let [pending] = page.items.as_slice() else {
        panic!("恰一轮待办");
    };
    let facts = pending.certify.clone().expect("付款腿认证事实");
    assert_eq!(order.collect_with(&Collector::signed()).await.advanced, 3);
    let evidence = CollectedEvidence {
        remote_observation_id: pending.sent_mid.clone(),
        evidence_digest: collected_digest(&pending.sent_mid),
    };
    let entries = order.count("settlement_funding_entries").await;
    let certify = |evidence: &CollectedEvidence| {
        let command = certify_cod_command(facts.clone(), evidence, Some(cash(BALANCE)));
        order.runtime.certify_cod(command)
    };

    // 正例：原输入重放读原结果。
    assert!(certify(&evidence).await.expect("重放").replayed);
    // 反例：同一业务键换付款证据摘要或远端观察身份，都是异请求冲突。
    let swapped_digest = CollectedEvidence { evidence_digest: [0xee; 32], ..evidence.clone() };
    assert!(matches!(certify(&swapped_digest).await, Err(FundingError::Conflict)));
    let swapped_id =
        CollectedEvidence { remote_observation_id: "9999999".to_owned(), ..evidence.clone() };
    assert!(matches!(certify(&swapped_id).await, Err(FundingError::Conflict)));
    assert_eq!(order.count("settlement_funding_entries").await, entries);
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
}

#[tokio::test]
async fn postgres_consignment_collection_concurrent_runtimes_advance_once() {
    // 对照：单个推进循环跑完同一形状订单的分录数。
    let single = dispatched_main().await;
    assert_eq!(single.collect_with(&Collector::signed()).await.advanced, 3);
    let expected = single.count("settlement_funding_entries").await;

    let order = dispatched_main().await;
    let (left, right) = (Collector::signed(), Collector::signed());
    let (first, second) = tokio::join!(order.collect_with(&left), order.collect_with(&right));
    assert_eq!(first.advanced + second.advanced, 3, "证据、认证、确领各首次提交一次");
    assert_eq!(
        order
            .scalar::<i64>(
                "SELECT count(*) FROM public.consignment_dispatch_evidence
                 WHERE category = 'original_claim'"
                    .to_owned()
            )
            .await,
        1
    );
    assert_eq!(order.count("consignment_cod_receipts").await, 1);
    assert_eq!(order.count("consignment_handoff_proofs").await, 1);
    assert_eq!(order.count("settlement_funding_entries").await, expected);
    assert_eq!(order.business_state("seller_payout").await, "ready");
    assert_eq!(order.posts(), 1);
}
