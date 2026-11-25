//! 第 6 期订单与购买 wire 契约的共用夹具、命令与版本绑定、金额守恒与公开字段白名单。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::AdminOrderV2;
use auction_contracts::ApiResponse;
use auction_contracts::ListingPage;
use auction_contracts::ListingSummaryV2;
use auction_contracts::OrderEvidencePageV2;
use auction_contracts::OrderPageV2;
use auction_contracts::OrderV2;
use auction_contracts::PurchaseReceiptV2;
use auction_contracts::PurchaseRejectionCodeV2;
use auction_contracts::PurchaseRequestV2;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;

macro_rules! fixture {
    ($dir:literal, $name:literal) => {
        include_str!(concat!("fixtures/", $dir, "/consignment-", $name, ".json"))
    };
}

fn decode<T: DeserializeOwned>(fixture: &str) -> T {
    serde_json::from_str(fixture).expect("订单夹具应可解码")
}

fn data(fixture: &str) -> Value {
    decode::<Value>(fixture)["data"].clone()
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str, fixture: &str) {
    let expected: Value = decode(fixture);
    let actual = serde_json::to_value(decode::<T>(fixture)).expect("DTO 应可序列化");
    assert!(actual == expected, "夹具 {name} 的往返结果发生变化");
}

fn rejects<T: DeserializeOwned>(wire: Value, scenario: &str) {
    assert!(serde_json::from_value::<T>(wire).is_err(), "应拒绝 {scenario}");
}

fn accepts<T: DeserializeOwned>(wire: Value, scenario: &str) {
    if let Err(error) = serde_json::from_value::<T>(wire) {
        panic!("应接受 {scenario}：{error}");
    }
}

fn with(object: &Value, changes: Value) -> Value {
    let mut changed = object.clone();
    for (key, value) in changes.as_object().unwrap() {
        if value.is_null() && key.starts_with('-') {
            changed.as_object_mut().unwrap().remove(&key[1..]);
        } else {
            changed[key] = value.clone();
        }
    }
    changed
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn buyer() -> Value {
    data(fixture!("public", "order-v2-buyer"))
}

fn seller() -> Value {
    data(fixture!("public", "order-v2-seller"))
}

fn withdrawal() -> Value {
    data(fixture!("public", "order-v2-withdrawal"))
}

#[test]
fn order_and_purchase_fixtures_round_trip() {
    round_trip::<PurchaseRequestV2>(
        "purchase-request-v2",
        fixture!("public", "purchase-request-v2"),
    );
    round_trip::<ApiResponse<PurchaseReceiptV2>>(
        "purchase-receipt-v2",
        fixture!("public", "purchase-receipt-v2"),
    );
    for (name, fixture) in [
        ("order-v2-buyer", fixture!("public", "order-v2-buyer")),
        ("order-v2-seller", fixture!("public", "order-v2-seller")),
        ("order-v2-withdrawal", fixture!("public", "order-v2-withdrawal")),
    ] {
        round_trip::<ApiResponse<OrderV2>>(name, fixture);
    }
    round_trip::<ApiResponse<OrderPageV2>>("orders-v2", fixture!("public", "orders-v2"));
    round_trip::<ApiResponse<ListingPage<ListingSummaryV2>>>(
        "listings-v2",
        fixture!("public", "listings-v2"),
    );
    round_trip::<ApiResponse<AdminOrderV2>>("admin order-v2", fixture!("admin", "order-v2"));
    round_trip::<ApiResponse<OrderEvidencePageV2>>(
        "admin order-v2-evidence",
        fixture!("admin", "order-v2-evidence"),
    );
}

#[test]
fn purchase_receipt_is_bound_to_command_id_price_and_version() {
    let request: PurchaseRequestV2 = decode(fixture!("public", "purchase-request-v2"));
    let receipt: ApiResponse<PurchaseReceiptV2> = decode(fixture!("public", "purchase-receipt-v2"));
    let receipt = receipt.data;
    assert!(receipt.answers(&request).is_ok());

    let wire = data(fixture!("public", "purchase-receipt-v2"));
    let other_command: PurchaseReceiptV2 = serde_json::from_value(with(
        &wire,
        json!({"command_id": "00000000-0000-4000-8000-00000000ffff"}),
    ))
    .unwrap();
    assert!(other_command.answers(&request).is_err(), "命令 ID 不同的回执不能回答该请求");
    let other_price: PurchaseReceiptV2 =
        serde_json::from_value(with(&wire, json!({"price": 102}))).unwrap();
    assert!(other_price.answers(&request).is_err());
    for stale in [5, 4, 1] {
        let not_advanced: PurchaseReceiptV2 =
            serde_json::from_value(with(&wire, json!({"listing_version": stale}))).unwrap();
        assert!(not_advanced.answers(&request).is_err(), "挂单版本 {stale} 没有越过预期版本 5");
    }
    // 重放返回原回执，版本比预期版本多走几步仍是同一回执。
    let replayed: PurchaseReceiptV2 =
        serde_json::from_value(with(&wire, json!({"listing_version": 9}))).unwrap();
    assert!(replayed.answers(&request).is_ok());

    for (changes, scenario) in [
        (json!({"schema_version": 1}), "旧 schema"),
        (json!({"state": "available"}), "挂单未进入 sale_pending"),
        (json!({"order_state": "awaiting_handoff"}), "购买后订单态不是 awaiting_payment"),
        (json!({"disposition": "closed"}), "非 purchased 处置"),
        (json!({"operation": "withdraw"}), "非 purchase 操作"),
        (json!({"contract_version": 0}), "合同版本为零"),
        (json!({"price": 0}), "价格越界"),
        (json!({"-schema_version": null}), "缺 schema_version"),
        (json!({"seller_name": "FixtureSeller"}), "回执夹带卖家信息"),
    ] {
        rejects::<PurchaseReceiptV2>(with(&wire, changes), scenario);
    }
}

#[test]
fn purchase_request_requires_uuid_v4_version_and_price() {
    let request = data_request();
    accepts::<PurchaseRequestV2>(request.clone(), "夹具请求");
    for (changes, scenario) in [
        (json!({"command_id": "00000000-0000-5000-8000-000000000510"}), "非 v4 命令 ID"),
        (json!({"expected_version": 0}), "版本为零"),
        (json!({"expected_price": 0}), "价格为零"),
        (json!({"expected_price": 1_000_000_001_i64}), "价格超过十亿"),
        (json!({"-expected_price": null}), "缺预期价"),
        (json!({"buyer_user_id": "00000000-0000-4000-8000-000000000521"}), "请求夹带买家身份"),
    ] {
        rejects::<PurchaseRequestV2>(with(&request, changes), scenario);
    }
}

fn data_request() -> Value {
    decode(fixture!("public", "purchase-request-v2"))
}

#[test]
fn seller_amounts_are_conserved_and_in_range() {
    accepts::<OrderV2>(seller(), "P=101 F=5 S=96");
    // F=0：价格 1 时手续费向下取整为零，卖方拿到全额。
    accepts::<OrderV2>(with(&seller(), json!({"price": 1, "fee": 0, "seller_amount": 1})), "F=0");
    for (changes, scenario) in [
        (json!({"fee": 6}), "F+S 大于 P"),
        (json!({"seller_amount": 95}), "F+S 小于 P"),
        (json!({"fee": 101, "seller_amount": 1}), "手续费不得吞没全部价款"),
        (json!({"fee": 101, "seller_amount": 0}), "到手金额为零"),
        (json!({"fee": -1, "seller_amount": 102}), "手续费为负"),
        (json!({"price": 0, "fee": 0, "seller_amount": 0}), "价格为零"),
        (
            json!({"price": 1_000_000_001_i64, "fee": 5, "seller_amount": 999_999_996}),
            "价格超过十亿",
        ),
        (json!({"-fee": null}), "卖方缺手续费"),
        (json!({"-seller_amount": null}), "卖方缺到手金额"),
        (json!({"fee": null}), "手续费为 null 而不是省略"),
        (json!({"-price": null}), "成交订单缺价格键"),
        (json!({"price": null}), "成交订单价格为 null"),
        (json!({"deadline_at": null}), "成交订单缺付款期限"),
    ] {
        rejects::<OrderV2>(with(&seller(), changes), scenario);
    }
    for (changes, scenario) in [
        (json!({"fee": 5, "seller_amount": 96}), "买方视图带私有金额"),
        (json!({"fee": 5}), "买方视图只带手续费"),
    ] {
        rejects::<OrderV2>(with(&buyer(), changes), scenario);
    }
    for (changes, scenario) in [
        (json!({"price": 101}), "撤回订单带价格"),
        (json!({"deadline_at": "2026-11-28T18:10:00Z"}), "撤回订单带付款期限"),
        (json!({"fee": 0}), "撤回订单带手续费"),
        (json!({"role": "buyer"}), "撤回订单对买家可见"),
    ] {
        rejects::<OrderV2>(with(&withdrawal(), changes), scenario);
    }
    // Admin 同时看到买卖双方，守恒规则相同。
    let admin = data(fixture!("admin", "order-v2"));
    for (changes, scenario) in [
        (json!({"fee": 6}), "Admin F+S 大于 P"),
        (json!({"fee": 101, "seller_amount": 1}), "Admin 手续费吞没价款"),
        (json!({"buyer_user_id": "00000000-0000-4000-8000-000000000522"}), "买家与卖家同账号"),
        (json!({"buyer_user_id": null}), "成交缺买家"),
    ] {
        rejects::<AdminOrderV2>(with(&admin, changes), scenario);
    }
}

#[test]
fn public_dtos_expose_only_whitelisted_fields() {
    let order_keys = [
        "allowed_actions",
        "contract_id",
        "deadline_at",
        "delivery_stage",
        "equipment_id",
        "kind",
        "listing_id",
        "order_state",
        "payment_stage",
        "payout_stage",
        "price",
        "refund_stage",
        "role",
        "version",
        "world",
    ];
    assert_eq!(keys(&buyer()), order_keys, "买方视图不含 fee 与 seller_amount");
    assert_eq!(keys(&withdrawal()), order_keys);
    let mut seller_keys = order_keys.to_vec();
    seller_keys.extend(["fee", "seller_amount"]);
    seller_keys.sort_unstable();
    assert_eq!(keys(&seller()), seller_keys);
    assert_eq!(
        keys(&data(fixture!("public", "purchase-receipt-v2"))),
        [
            "command_id",
            "contract_id",
            "contract_version",
            "disposition",
            "equipment_id",
            "listing_id",
            "listing_version",
            "operation",
            "order_state",
            "price",
            "schema_version",
            "state",
            "world",
        ]
    );
    // 目录摘要沿用 v2 白名单：无卖家账号、预约码、装备 key。
    let catalog = data(fixture!("public", "listings-v2"));
    assert_eq!(
        keys(&catalog["items"][0]),
        [
            "allowed_actions",
            "category",
            "effective_reason",
            "expires_at",
            "external_equipment_id",
            "generation",
            "level",
            "listing_id",
            "mode",
            "name",
            "price",
            "purchasable",
            "seller_name",
            "state",
            "updated_at",
            "version",
            "world_id",
        ]
    );

    for private in [
        "buyer_user_id",
        "seller_user_id",
        "buyer_name",
        "reservation_code",
        "equipment_key",
        "external_mid",
        "mail_body",
    ] {
        for (name, wire) in [("buyer", buyer()), ("seller", seller()), ("withdrawal", withdrawal())]
        {
            rejects::<OrderV2>(
                with(&wire, json!({ private: "x" })),
                &format!("{name} 订单夹带 {private}"),
            );
        }
        let receipt = data(fixture!("public", "purchase-receipt-v2"));
        rejects::<PurchaseReceiptV2>(with(&receipt, json!({ private: "x" })), private);
        let mut item = data(fixture!("public", "listings-v2"))["items"][0].clone();
        item[private] = json!("x");
        rejects::<ListingSummaryV2>(item, private);
    }
    // 目录只能宣告购买动作，且只有 available 可购买。
    let item = data(fixture!("public", "listings-v2"))["items"][0].clone();
    accepts::<ListingSummaryV2>(item.clone(), "available 且可购买");
    rejects::<ListingSummaryV2>(with(&item, json!({"allowed_actions": ["withdraw"]})), "私有动作");
    rejects::<ListingSummaryV2>(
        with(&item, json!({"state": "awaiting_deposit", "generation": null})),
        "未代管却可购买",
    );
}

#[test]
fn stage_matrix_is_shared_with_the_domain_projection() {
    let matrix: Value = decode(fixture!("public", "order-v2-stage-matrix"));
    let template = |kind: &str| match kind {
        "withdrawal" => withdrawal(),
        _ => seller(),
    };
    let stamped = |kind: &str, stages: &Value| {
        let mut wire = template(kind);
        for (key, value) in stages.as_object().unwrap() {
            wire[key] = value.clone();
        }
        wire
    };
    for case in matrix["cases"].as_array().unwrap() {
        let kind = case["facts"]["kind"].as_str().unwrap();
        accepts::<OrderV2>(stamped(kind, &case["stages"]), case["name"].as_str().unwrap());
    }
    for case in matrix["invalid"].as_array().unwrap() {
        let kind = case["kind"].as_str().unwrap();
        rejects::<OrderV2>(stamped(kind, &case["stages"]), case["name"].as_str().unwrap());
    }
    // 第 6 期订单没有可执行动作。
    rejects::<OrderV2>(with(&buyer(), json!({"allowed_actions": ["claim"]})), "第 6 期出现动作");
    rejects::<OrderV2>(with(&buyer(), json!({"order_state": "paid"})), "未知订单态");
}

#[test]
fn purchase_rejection_codes_match_the_shared_enums() {
    let enums: Value = decode(fixture!("public", "order-enums-v2"));
    for entry in enums["purchase_rejections"].as_array().unwrap() {
        let code: PurchaseRejectionCodeV2 = serde_json::from_value(entry["code"].clone()).unwrap();
        assert_eq!(code.http_status(), entry["status"].as_i64().unwrap(), "{}", entry["code"]);
    }
    assert!(PurchaseRejectionCodeV2::FundingUnavailable.is_trusted_before_write());
    assert!(!PurchaseRejectionCodeV2::OutcomeUnknown.is_trusted_before_write());
    assert!(PurchaseRejectionCodeV2::OutcomeUnknown.is_outcome_unknown());
    assert!(!PurchaseRejectionCodeV2::FundingUnavailable.is_outcome_unknown());
    assert!(serde_json::from_value::<PurchaseRejectionCodeV2>(json!("mail_unavailable")).is_err());
}

#[test]
fn admin_evidence_rejects_inconsistent_items() {
    let page = data(fixture!("admin", "order-v2-evidence"));
    let item = |index: usize| page["items"][index].clone();
    let evidence = |index: usize, changes: Value| with(&item(index), changes);
    rejects::<OrderEvidencePageV2>(
        with(&page, json!({"items": [evidence(1, json!({"write_started": false}))]})),
        "起写标记与阶段不一致",
    );
    rejects::<OrderEvidencePageV2>(
        with(&page, json!({"items": [evidence(2, json!({"fence": 1}))]})),
        "未起写的接管带 fence",
    );
    rejects::<OrderEvidencePageV2>(
        with(&page, json!({"items": [evidence(2, json!({"state": "write_started"}))]})),
        "已起写的接管缺 fence",
    );
    rejects::<OrderEvidencePageV2>(
        with(&page, json!({"items": [evidence(5, json!({"mail_body": "x"}))]})),
        "证据夹带原信正文",
    );
    rejects::<OrderEvidencePageV2>(
        with(&page, json!({"items": [evidence(3, json!({"digest": "A".repeat(64)}))]})),
        "摘要不是小写十六进制",
    );
}
