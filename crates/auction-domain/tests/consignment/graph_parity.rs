//! 合同图判定与 SQL `c5_check_contract_graph` 的对照：同一份 JSON 夹具，两边结论必须一致。
//!
//! 夹具由 `crates/auction-storage/tests/postgres/consignment/contracts/graph_parity.rs` 在真实 PG 上
//! 以同名案例重放；不一致时以本侧领域规则为准修 SQL（D9 §10）。

use auction_domain::SettlementObligationKind::{
    BuyerRefund, SellerPayout, WinnerEquipmentCod, WinnerPaymentCarrierCod,
};
use auction_domain::consignment::{
    ConsignmentContract, ConsignmentObligationGraph, validate_consignment_graph,
};
use auction_domain::{SettlementAmount, SettlementObligationState, SettlementWorld};

use super::contract_vectors::{graph, must, sale, withdrawal};

const CASES: &str = include_str!("../fixtures/contract_graph_cases.json");

/// 按夹具的变异名篡改一份合法图；名称与 PG 侧的 `Graph` 旋钮一一对应。
fn mutate(graph: &mut ConsignmentObligationGraph, mutation: &str) {
    let amount = |value: i64| must(SettlementAmount::new(value));
    match mutation {
        "none" => {}
        "drop_refund" => graph.obligations.retain(|item| item.kind != BuyerRefund),
        "payout_amount_minus_one" => {
            for item in graph.obligations.iter_mut().filter(|item| item.kind == SellerPayout) {
                item.amount = amount(item.amount.value() - 1);
            }
        }
        "payout_identity_other" => {
            for item in graph.obligations.iter_mut().filter(|item| item.kind == SellerPayout) {
                item.recipient_game_identity = "SomeoneElse".to_owned();
            }
        }
        "cod_amount_plus_one" => {
            for item in graph
                .obligations
                .iter_mut()
                .filter(|item| matches!(item.kind, WinnerEquipmentCod | WinnerPaymentCarrierCod))
            {
                item.amount = amount(item.amount.value() + 1);
            }
        }
        "refund_state_ready" => {
            for item in graph.obligations.iter_mut().filter(|item| item.kind == BuyerRefund) {
                item.state = SettlementObligationState::Ready;
            }
        }
        other => panic!("未知变异 {other}"),
    }
}

#[test]
fn domain_graph_verdicts_match_the_shared_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(CASES).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let world = match case["world"].as_str().unwrap() {
            "main" => SettlementWorld::Main,
            _ => SettlementWorld::Isekai,
        };
        let contract: ConsignmentContract = match case["kind"].as_str().unwrap() {
            "sale" => sale(world, 1000, 500),
            _ => withdrawal(world),
        };
        let mut candidate = graph(&contract);
        mutate(&mut candidate, case["mutation"].as_str().unwrap());
        let verdict = validate_consignment_graph(&candidate).is_ok();
        assert_eq!(verdict, case["valid"].as_bool().unwrap(), "{name}");
    }
}
