//! 合同图判定对照：与领域 `graph_parity` 单测读同一份 JSON 夹具，SQL 触发器必须给出同一结论（不变量 6）。

use serde::Deserialize;

use super::super::deposit::support::{DECIDE, fails, succeeds};
use super::fixture::{Graph, seed_isekai_custody, seed_main_custody};
use crate::support::TestDatabase;

const CASES: &str =
    include_str!("../../../../../auction-domain/tests/fixtures/contract_graph_cases.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    kind: String,
    world: String,
    mutation: String,
    valid: bool,
}

/// 把夹具的变异名翻译成 `Graph` 旋钮；名称与领域侧 `mutate` 一一对应。
fn graph(case: &Case) -> Graph {
    let base = match (case.kind.as_str(), case.world.as_str()) {
        ("withdrawal", _) => Graph::withdrawal(),
        (_, "isekai") => Graph::sale_isekai(),
        _ => Graph::sale_main(),
    };
    match case.mutation.as_str() {
        "none" => base,
        "drop_refund" => Graph { with_refund: false, ..base },
        "payout_amount_minus_one" => Graph { payout_amount: base.payout_amount - 1, ..base },
        "payout_identity_other" => Graph { payout_identity: "SomeoneElse", ..base },
        "cod_amount_plus_one" => Graph { cod_amount: Some(base.price + 1), ..base },
        "refund_state_ready" => Graph { refund_state: "ready", ..base },
        other => panic!("未知变异 {other}"),
    }
}

#[tokio::test]
async fn postgres_consignment_contract_graph_verdicts_match_domain_fixture() {
    let fixture: Fixture = serde_json::from_str(CASES).expect("解析对照夹具");
    assert!(
        fixture.cases.iter().any(|case| case.valid) && fixture.cases.iter().any(|case| !case.valid)
    );
    for case in &fixture.cases {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let pool = owner.postgres_pool();
        if case.world == "isekai" {
            seed_isekai_custody(pool, "transaction_timestamp() + interval '10 days'").await;
        } else {
            seed_main_custody(pool).await;
        }
        let sql = graph(case).sql(DECIDE);
        if case.valid {
            succeeds(pool, &sql).await;
        } else {
            fails(pool, &sql, "23514", Some("c5_contract_graph")).await;
        }
    }
}
