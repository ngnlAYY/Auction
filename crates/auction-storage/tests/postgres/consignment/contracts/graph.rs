//! 合同图完整性：合同、子项、义务与履约行在提交前必须整体成形，缺一即整体回滚（不变量 3、6）。

use sqlx::PgPool;

use super::super::deposit::support::{DECIDE, fails, succeeds};
use super::fixture::{Graph, seed_isekai_custody, seed_main_custody};
use crate::support::TestDatabase;

async fn graph_rows(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_contracts),
                (SELECT count(*) FROM public.consignment_sales)
                    + (SELECT count(*) FROM public.consignment_withdrawals),
                (SELECT count(*) FROM public.settlement_obligations WHERE contract_id IS NOT NULL),
                (SELECT count(*) FROM public.settlement_obligation_fulfillments AS fulfillment
                 JOIN public.settlement_obligations AS obligation
                   ON obligation.id = fulfillment.obligation_id
                 WHERE obligation.contract_id IS NOT NULL)",
    )
    .fetch_one(pool)
    .await
    .expect("读取合同图行数")
}

#[tokio::test]
async fn postgres_consignment_contract_graph_commits_each_fixed_shape() {
    // 正例：Sale(main)=3 条、Sale(isekai)=4 条、Withdrawal=1 条义务，每条义务一行履约。
    for (graph, isekai, expected) in [
        (Graph::sale_main(), false, (1, 1, 3, 3)),
        (Graph::sale_isekai(), true, (1, 1, 4, 4)),
        (Graph::withdrawal(), false, (1, 1, 1, 1)),
    ] {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let pool = owner.postgres_pool();
        if isekai {
            seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
        } else {
            seed_main_custody(pool).await;
        }
        succeeds(pool, &graph.sql(DECIDE)).await;
        assert_eq!(graph_rows(pool).await, expected, "{} {}", graph.kind, graph.world);
    }
}

#[tokio::test]
async fn postgres_consignment_contract_graph_rejects_incomplete_or_mismatched_graphs() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;

    // 反例：缺退款义务、缺退款履约、卖款金额不等于 S、卖款收款人不是冻结卖家，提交时整体拒绝。
    let cases = [
        Graph { with_refund: false, ..Graph::sale_main() },
        Graph { refund_fulfillment: false, ..Graph::sale_main() },
        Graph { payout_amount: 949, ..Graph::sale_main() },
        Graph { payout_identity: "SomeoneElse", ..Graph::sale_main() },
    ];
    for graph in cases {
        fails(pool, &graph.sql(DECIDE), "23514", Some("c5_contract_graph")).await;
        // 原子性：失败的提交不留下任何合同、子项、义务或履约行。
        assert_eq!(graph_rows(pool).await, (0, 0, 0, 0));
    }
    // 反例：只有合同头、没有任何义务。
    fails(pool, &Graph::sale_main().head_sql(DECIDE), "23514", Some("c5_contract_graph")).await;
    assert_eq!(graph_rows(pool).await, (0, 0, 0, 0));
}
