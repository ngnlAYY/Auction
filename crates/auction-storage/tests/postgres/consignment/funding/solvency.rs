//! 寄售偿付口径：一笔 funded 成交的最大负债取 P，不取 S+P；未付款的成交不计负债。

use auction_application::reconciliation::AssetReconciliationStore;

use super::cod_fixture::{Shape, certify, seed};
use crate::support::TestDatabase;

const PRICE: i64 = 101;

async fn main_report(
    store: &auction_storage::DatabaseStore,
) -> auction_application::reconciliation::SolvencyWorldReport {
    let report = store.report_solvency(3_600).await.expect("生成偿付报告");
    assert!(report.complete, "偿付查询必须执行成功");
    report.worlds.into_iter().find(|world| world.world == "main").expect("main 报告")
}

#[tokio::test]
async fn postgres_consignment_solvency_max_liability_is_price_not_seller_plus_price() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let store = database.store().await.expect("以运行账号连接");
    seed(owner.postgres_pool(), Shape::STRICT).await;

    // 反例：成交尚未收款（无 funded 投影），Bot 没有持有买家款项，不计任何负债。
    let unpaid = main_report(&store).await;
    assert_eq!((unpaid.pending_seller_payout_credits, unpaid.pending_refund_credits), (0, 0));
    assert_eq!(unpaid.required_credits, 0);

    // 正例：收款认证后 funded：卖款 S=96 与买家退款 P=101 互斥，最大负债只计一份 P。
    store.test_certify_cod(certify(2, None)).await.expect("认证 COD");
    let funded = main_report(&store).await;
    assert_eq!(funded.required_credits, PRICE, "最大负债取 P，不取 S+P");
    assert_eq!(
        (funded.pending_refund_count, funded.pending_refund_credits),
        (1, PRICE),
        "互斥的两条腿只计一次"
    );
    assert_eq!((funded.pending_seller_payout_count, funded.pending_seller_payout_credits), (0, 0));
}
