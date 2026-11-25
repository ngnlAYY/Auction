//! 订单阶段投影的矩阵向量，与契约 crate 的 `OrderV2` 校验共用同一份夹具互核。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::consignment::{
    OrderDeliveryStage, OrderFacts, OrderKind, OrderLegStage, OrderOutcome, OrderPaymentStage,
    project_order_stages,
};
use serde_json::Value;

const MATRIX: &str = include_str!(
    "../../../auction-contracts/tests/fixtures/public/consignment-order-v2-stage-matrix.json"
);

fn kind(code: &str) -> OrderKind {
    match code {
        "sale" => OrderKind::Sale,
        "withdrawal" => OrderKind::Withdrawal,
        other => panic!("未知订单种类 {other}"),
    }
}

fn outcome(code: &str) -> OrderOutcome {
    match code {
        "open" => OrderOutcome::Open,
        "completed" => OrderOutcome::Completed,
        "cancelled" => OrderOutcome::Cancelled,
        other => panic!("未知结局 {other}"),
    }
}

fn delivery(code: &str) -> OrderDeliveryStage {
    [
        OrderDeliveryStage::NotStarted,
        OrderDeliveryStage::Prepared,
        OrderDeliveryStage::InFlight,
        OrderDeliveryStage::Sent,
        OrderDeliveryStage::Claimed,
        OrderDeliveryStage::Returned,
        OrderDeliveryStage::Closed,
        OrderDeliveryStage::Unknown,
    ]
    .into_iter()
    .find(|stage| stage.as_str() == code)
    .unwrap_or_else(|| panic!("未知交割阶段 {code}"))
}

fn payment(code: &str) -> OrderPaymentStage {
    [
        OrderPaymentStage::NotApplicable,
        OrderPaymentStage::Unpaid,
        OrderPaymentStage::Certified,
        OrderPaymentStage::Closed,
        OrderPaymentStage::Unknown,
    ]
    .into_iter()
    .find(|stage| stage.as_str() == code)
    .unwrap_or_else(|| panic!("未知付款阶段 {code}"))
}

fn leg(code: &str) -> OrderLegStage {
    [
        OrderLegStage::NotApplicable,
        OrderLegStage::Blocked,
        OrderLegStage::Ready,
        OrderLegStage::Reserved,
        OrderLegStage::Sent,
        OrderLegStage::Claimed,
        OrderLegStage::Returned,
        OrderLegStage::Closed,
        OrderLegStage::Unknown,
    ]
    .into_iter()
    .find(|stage| stage.as_str() == code)
    .unwrap_or_else(|| panic!("未知资金腿阶段 {code}"))
}

#[test]
fn projection_matches_every_shared_matrix_case() {
    let matrix: Value = serde_json::from_str(MATRIX).unwrap();
    let cases = matrix["cases"].as_array().unwrap();
    assert!(cases.len() >= 20, "矩阵不得被悄悄缩小");
    for case in cases {
        let facts = &case["facts"];
        let text = |key: &str| facts[key].as_str().unwrap();
        let projected = project_order_stages(&OrderFacts {
            kind: kind(text("kind")),
            outcome: outcome(text("outcome")),
            delivery: delivery(text("delivery")),
            payment: payment(text("payment")),
            payout: leg(text("payout")),
            refund: leg(text("refund")),
        });
        let expected = &case["stages"];
        let actual = [
            projected.order_state.as_str(),
            projected.delivery.as_str(),
            projected.payment.as_str(),
            projected.payout.as_str(),
            projected.refund.as_str(),
        ];
        let wanted = [
            expected["order_state"].as_str().unwrap(),
            expected["delivery_stage"].as_str().unwrap(),
            expected["payment_stage"].as_str().unwrap(),
            expected["payout_stage"].as_str().unwrap(),
            expected["refund_stage"].as_str().unwrap(),
        ];
        assert_eq!(actual, wanted, "{}", case["name"]);
    }
}

#[test]
fn projection_never_leaves_an_unknown_stage_outside_reconcile() {
    for slot in 0..4 {
        let mut facts = OrderFacts {
            kind: OrderKind::Sale,
            outcome: OrderOutcome::Open,
            delivery: OrderDeliveryStage::Sent,
            payment: OrderPaymentStage::Certified,
            payout: OrderLegStage::Blocked,
            refund: OrderLegStage::NotApplicable,
        };
        match slot {
            0 => facts.delivery = OrderDeliveryStage::Unknown,
            1 => facts.payment = OrderPaymentStage::Unknown,
            2 => facts.payout = OrderLegStage::Unknown,
            _ => facts.refund = OrderLegStage::Unknown,
        }
        assert_eq!(project_order_stages(&facts).order_state.as_str(), "reconcile");
    }
}
