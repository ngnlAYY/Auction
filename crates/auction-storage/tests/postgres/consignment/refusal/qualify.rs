//! 资格裁决：待判在新观察到来时重裁，不计与合格为终局；只有合格才写拒收。

use auction_domain::consignment::refusal::{NotCounted, PendingReason, Qualification};

use super::fixture::{count, good_evidence, seed_base, seed_intakes, submit_with};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_refusal_pending_is_reevaluated_and_not_counted_is_terminal() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_base(owner).await;
    seed_intakes(owner, 2).await;

    // 归属未证明：保留事实与待判裁决，不写拒收；相同证据再提交不追加重复裁决。
    let unproven = super::fixture::good_evidence();
    let unproven = auction_storage::ReturnEvidence { ownership_proven: false, ..unproven };
    let first = submit_with(&store, 1, unproven).await;
    assert_eq!(first.qualification, Qualification::Pending(PendingReason::OwnershipUnproven));
    assert_eq!(count(owner, "consignment_refusals").await, 0);
    let again = submit_with(&store, 1, unproven).await;
    assert!(again.replayed);
    assert_eq!(count(owner, "consignment_return_fact_verdicts").await, 1);

    // 新观察证清归属：追加合格裁决并写拒收，事实本身不变。
    let proven = submit_with(&store, 1, good_evidence()).await;
    assert_eq!(proven.qualification, Qualification::Qualified);
    assert!(!proven.replayed);
    assert_eq!(count(owner, "consignment_return_facts").await, 1);
    assert_eq!(count(owner, "consignment_return_fact_verdicts").await, 2);
    assert_eq!(count(owner, "consignment_refusals").await, 1);

    // 已付款的退回不计，且为终局：之后即使证据变成“成立”也不会翻成合格。
    let paid = auction_storage::ReturnEvidence { paid_or_opposite: true, ..good_evidence() };
    let not_counted = submit_with(&store, 2, paid).await;
    assert_eq!(not_counted.qualification, Qualification::NotCounted(NotCounted::PaidOrOpposite));
    let retried = submit_with(&store, 2, good_evidence()).await;
    assert_eq!(retried.qualification, Qualification::NotCounted(NotCounted::PaidOrOpposite));
    assert!(retried.replayed);
    assert_eq!(count(owner, "consignment_refusals").await, 1);
}
