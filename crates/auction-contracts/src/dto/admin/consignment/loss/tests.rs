//! 损失案件 DTO 的核心断言：请求只收固定字段，回执与读模型不带任何金额字段。

use serde_json::{Value, json};

use super::{
    AdminLossCase, LossCaseOutcomeReceipt, RecordLossCaseOutcomeRequest, RecordLossProofRequest,
};

const COMMAND: &str = "00000000-0000-4000-8000-000000000001";
const CASE: &str = "00000000-0000-4000-8000-000000000002";
const BASIS: &str = "00000000-0000-4000-8000-000000000003";

fn request() -> Value {
    json!({
        "command_id": COMMAND,
        "case_id": CASE,
        "expected_case_version": 1,
        "outcome": "reviewing",
        "basis_id": BASIS,
        "private_note": "运营夹具：复核中",
    })
}

#[test]
fn request_rejects_money_fields_and_non_v4_command() {
    assert!(serde_json::from_value::<RecordLossCaseOutcomeRequest>(request()).is_ok());
    for extra in ["amount", "refund", "payout", "credits"] {
        let mut body = request();
        body[extra] = json!(1);
        assert!(
            serde_json::from_value::<RecordLossCaseOutcomeRequest>(body).is_err(),
            "{extra} 必须按未知字段拒绝"
        );
    }
    let mut body = request();
    body["command_id"] = json!("00000000-0000-1000-8000-000000000001");
    assert!(serde_json::from_value::<RecordLossCaseOutcomeRequest>(body).is_err());
}

#[test]
fn receipt_and_case_carry_no_money_fields() {
    let receipt: LossCaseOutcomeReceipt = serde_json::from_value(json!({
        "schema_version": 2,
        "operation": "record_loss_case_outcome",
        "disposition": "case_outcome_recorded",
        "command_id": COMMAND,
        "case_id": CASE,
        "case_version": 2,
        "outcome_id": "00000000-0000-4000-8000-000000000004",
        "seller_receipt_id": "00000000-0000-4000-8000-000000000005",
    }))
    .expect("回执");
    let value = serde_json::to_value(&receipt).expect("编码");
    let mut keys: Vec<_> = value.as_object().expect("对象").keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "case_id",
            "case_version",
            "command_id",
            "disposition",
            "operation",
            "outcome_id",
            "schema_version",
            "seller_receipt_id",
        ]
    );
    let mut with_amount = value;
    with_amount["amount"] = json!(1);
    assert!(serde_json::from_value::<LossCaseOutcomeReceipt>(with_amount).is_err());

    let case = json!({
        "position_id": COMMAND, "case_id": CASE,
        "loss_fact_id": "00000000-0000-4000-8000-000000000006",
        "proof_id": "00000000-0000-4000-8000-000000000007",
        "proof_source_kind": "fake-source", "proof_digest": "a".repeat(64),
        "listing_id": "00000000-0000-4000-8000-000000000008",
        "seller_user_id": "00000000-0000-4000-8000-000000000009",
        "generation": 1, "case_version": 1, "state": "opened",
        "outcomes": [{
            "outcome_id": "00000000-0000-4000-8000-000000000004", "case_version": 1,
            "state": "opened", "basis_id": null, "private_note": null,
            "occurred_at": "2026-11-21T00:00:00Z",
            "seller_receipt_id": "00000000-0000-4000-8000-000000000005",
        }],
    });
    assert!(serde_json::from_value::<AdminLossCase>(case.clone()).is_ok());
    let mut with_refund = case;
    with_refund["refund_amount"] = json!(1);
    assert!(serde_json::from_value::<AdminLossCase>(with_refund).is_err());
}

/// 灭失证据请求：派生摘要与未知字段整体拒绝；观察身份与最后确定位置拒装备链接与 key 形状；
/// Debug 不输出证据文本与私有说明（不变量 6、7）。
#[test]
fn loss_proof_request_rejects_secret_shapes_and_derived_digests() {
    let request = || {
        json!({
            "command_id": COMMAND,
            "source_kind": "fake-source",
            "observation_ref": "fake-observation-1",
            "last_known_ref": "夹具：Bot 收件箱",
            "private_note": "运营夹具：录入证据",
        })
    };
    let parsed = serde_json::from_value::<RecordLossProofRequest>(request()).expect("证据请求");
    let debug = format!("{parsed:?}");
    assert!(!debug.contains("fake-observation-1") && !debug.contains("运营夹具"));
    for (field, value) in [
        ("observation_ref", "https://hentaiverse.example.invalid/equip/1/0123456789"),
        ("observation_ref", "seen 0123456789"),
        ("last_known_ref", "key=0123456789"),
    ] {
        let mut body = request();
        body[field] = json!(value);
        assert!(serde_json::from_value::<RecordLossProofRequest>(body).is_err(), "{field}");
    }
    for extra in ["digest", "movement_digest", "exclusion_digest"] {
        let mut body = request();
        body[extra] = json!("a".repeat(64));
        assert!(serde_json::from_value::<RecordLossProofRequest>(body).is_err(), "{extra}");
    }
}
