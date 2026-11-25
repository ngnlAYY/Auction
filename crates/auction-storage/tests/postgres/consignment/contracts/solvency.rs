//! 偿付报表只算拍卖义务：寄售的卖款与退款由第 6 期 6C 的专用口径计入（寄售一笔交易的最大负债取
//! 买家付款额，卖款与退款互斥，不能沿用拍卖口径把两者相加）。

use auction_application::consignment::ConsignmentGraphStore;
use auction_application::reconciliation::AssetReconciliationStore;
use auction_domain::SettlementWorld;

use super::fixture::seed_main_custody;
use super::graph_write::{sale_graph, seed_funding};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_obligations_do_not_enter_auction_solvency_report() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    seed_funding(pool).await;
    runtime.write_graph(sale_graph(SettlementWorld::Main, 1000)).await.expect("写入合同图");
    // 卖款 blocked 未满足；把退款从 contingent 激活为 ready，使两者都满足拍卖口径的“待付”条件。
    // 或有义务激活要先清除履约的休眠标记（触发器要求两者一致）。
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments SET dormant = false
         WHERE obligation_id IN (SELECT id FROM public.settlement_obligations
                                 WHERE contract_id IS NOT NULL AND obligation_kind = 'buyer_refund')",
    )
    .execute(pool)
    .await
    .expect("清除休眠标记");
    sqlx::query(
        "UPDATE public.settlement_obligations SET business_state = 'ready'
         WHERE contract_id IS NOT NULL AND obligation_kind = 'buyer_refund'",
    )
    .execute(pool)
    .await
    .expect("激活退款义务");
    let unsatisfied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_obligations
         WHERE contract_id IS NOT NULL AND obligation_kind IN ('seller_payout', 'buyer_refund')
           AND business_state <> 'contingent'",
    )
    .fetch_one(pool)
    .await
    .expect("寄售卖款与退款数");
    assert_eq!(unsatisfied, 2, "前提：两条寄售义务都未满足且非或有");

    let solvency = runtime.report_solvency(3_600).await.expect("生成偿付报告");
    assert!(solvency.complete, "偿付查询必须执行成功");
    let main = solvency.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!(
        (
            main.pending_seller_payout_count,
            main.pending_seller_payout_credits,
            main.pending_refund_count,
            main.pending_refund_credits,
            main.in_transit_review_count,
            main.custody_count,
            main.required_credits,
        ),
        (0, 0, 0, 0, 0, 0, 0),
        "寄售义务不得进入拍卖偿付口径"
    );
}
