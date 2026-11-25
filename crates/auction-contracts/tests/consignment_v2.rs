//! 第 4 期 v2 wire 契约的共用夹具往返、严格性与脱敏回归。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::ApiResponse;
use auction_contracts::ConsignmentDelistReceiptV2;
use auction_contracts::ConsignmentDispositionV2;
use auction_contracts::ConsignmentHoldReceipt;
use auction_contracts::ConsignmentOperationV2;
use auction_contracts::ConsignmentStatus;
use auction_contracts::ConsignmentStatusV2;
use auction_contracts::DepositInstructions;
use auction_contracts::DepositReservationReceipt;
use auction_contracts::IssueDepositReservation;
use auction_contracts::ListingActionV2;
use auction_contracts::ListingDetailV2;
use auction_contracts::ListingReasonV2;
use auction_contracts::ListingStateV2;
use auction_contracts::ListingSummaryV2;
use auction_contracts::ListingViewV2;
use auction_contracts::OwnedListingV2;
use auction_contracts::PublicListingStateV2;
use auction_contracts::QuarantineAction;
use auction_contracts::QuarantineDecisionKind;
use auction_contracts::QuarantineDetail;
use auction_contracts::QuarantineObservationItemKind;
use auction_contracts::QuarantineReason;
use auction_contracts::QuarantineRecheckReceipt;
use auction_contracts::QuarantineRecheckRequest;
use auction_contracts::QuarantineReleaseReceipt;
use auction_contracts::QuarantineReleaseRequest;
use auction_contracts::QuarantineState;
use auction_contracts::RequestErrorCodeV2;
use auction_contracts::SeasonStateV2;
use auction_contracts::WithdrawListingV2;
use auction_contracts::WithdrawReceiptV2;
use auction_contracts::WriteRejectionCodeV2;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;

macro_rules! public_fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/public/consignment-", $name, ".json"))
    };
}

macro_rules! admin_fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/admin/consignment-", $name, ".json"))
    };
}

fn decode<T: DeserializeOwned>(fixture: &str) -> T {
    serde_json::from_str(fixture).unwrap_or_else(|error| {
        panic!("寄售 v2 夹具解码失败（{:?}，第 {} 行）", error.classify(), error.line())
    })
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str, fixture: &str) {
    let expected: Value = decode(fixture);
    let decoded: T = decode(fixture);
    let actual = serde_json::to_value(decoded).expect("寄售 v2 DTO 应可序列化");
    assert!(actual == expected, "夹具 {name} 的往返结果发生变化");
}

fn assert_rejects<T: DeserializeOwned>(wire: Value, scenario: &str) {
    assert!(serde_json::from_value::<T>(wire).is_err(), "应拒绝 {scenario}");
}

fn with_field(object: &Value, field: &str, value: Value) -> Value {
    let mut changed = object.clone();
    changed[field] = value;
    changed
}

fn without_field(object: &Value, field: &str) -> Value {
    let mut changed = object.clone();
    assert!(changed.as_object_mut().unwrap().remove(field).is_some());
    changed
}

fn public_status() -> Value {
    decode(public_fixture!("status-v2"))
}

fn public_status_data() -> Value {
    public_status()["data"].clone()
}

fn active_instructions() -> Value {
    decode::<Value>(public_fixture!("deposit-instructions-active"))["data"].clone()
}

fn owned_available() -> Value {
    decode::<Value>(public_fixture!("listing-v2-owned"))["data"].clone()
}

fn owned_held() -> Value {
    decode::<Value>(public_fixture!("listing-v2-held"))["data"].clone()
}

fn quarantine_active() -> Value {
    decode::<Value>(admin_fixture!("quarantine-active"))["data"].clone()
}

fn quarantine_released() -> Value {
    decode::<Value>(admin_fixture!("quarantine-released"))["data"].clone()
}

#[test]
fn public_v2_fixtures_round_trip() {
    for (name, fixture) in [
        ("status-v2", public_fixture!("status-v2")),
        ("status-v2-unknown", public_fixture!("status-v2-unknown")),
        ("status-v2-disabled", public_fixture!("status-v2-disabled")),
    ] {
        round_trip::<ApiResponse<ConsignmentStatusV2>>(name, fixture);
    }
    for (name, fixture) in [
        ("deposit-instructions-none", public_fixture!("deposit-instructions-none")),
        ("deposit-instructions-active", public_fixture!("deposit-instructions-active")),
    ] {
        round_trip::<ApiResponse<DepositInstructions>>(name, fixture);
    }
    round_trip::<IssueDepositReservation>(
        "deposit-reservation-request",
        public_fixture!("deposit-reservation-request"),
    );
    round_trip::<WithdrawListingV2>(
        "withdrawal-request-v2",
        public_fixture!("withdrawal-request-v2"),
    );
    round_trip::<ApiResponse<DepositReservationReceipt>>(
        "deposit-reservation-receipt",
        public_fixture!("deposit-reservation-receipt"),
    );
    round_trip::<ApiResponse<WithdrawReceiptV2>>(
        "withdrawal-requested-receipt",
        public_fixture!("withdrawal-requested-receipt"),
    );
    round_trip::<ApiResponse<WithdrawReceiptV2>>(
        "withdraw-closed-receipt",
        public_fixture!("withdraw-closed-receipt"),
    );
    round_trip::<ApiResponse<ListingDetailV2>>(
        "listing-v2-public",
        public_fixture!("listing-v2-public"),
    );
    round_trip::<ApiResponse<OwnedListingV2>>(
        "listing-v2-owned",
        public_fixture!("listing-v2-owned"),
    );
    round_trip::<ApiResponse<OwnedListingV2>>(
        "listing-v2-held",
        public_fixture!("listing-v2-held"),
    );
}

fn assert_v2_receipt_schema<T: DeserializeOwned>(wire: Value, name: &str) {
    for version in [json!(1), json!(3)] {
        assert_rejects::<T>(
            with_field(&wire, "schema_version", version),
            &format!("{name} 的 schema_version 不是 2"),
        );
    }
    let mut missing = wire;
    assert!(missing.as_object_mut().unwrap().remove("schema_version").is_some());
    assert_rejects::<T>(missing, &format!("{name} 缺少 schema_version"));
}

#[test]
fn every_v2_receipt_requires_schema_version_two() {
    assert_v2_receipt_schema::<DepositReservationReceipt>(
        decode::<Value>(public_fixture!("deposit-reservation-receipt"))["data"].clone(),
        "预约回执",
    );
    assert_v2_receipt_schema::<WithdrawReceiptV2>(
        decode::<Value>(public_fixture!("withdrawal-requested-receipt"))["data"].clone(),
        "撤回意愿回执",
    );
    assert_v2_receipt_schema::<WithdrawReceiptV2>(
        decode::<Value>(public_fixture!("withdraw-closed-receipt"))["data"].clone(),
        "未代管撤回回执",
    );
    assert_v2_receipt_schema::<QuarantineRecheckReceipt>(
        decode::<Value>(admin_fixture!("quarantine-recheck-receipt"))["data"].clone(),
        "隔离重查回执",
    );
    for fixture in [
        admin_fixture!("quarantine-release-accept"),
        admin_fixture!("quarantine-release-return"),
        admin_fixture!("quarantine-release-archive-empty"),
        admin_fixture!("quarantine-release-acquire-return"),
        admin_fixture!("quarantine-release-auction-return-case-unbound"),
        admin_fixture!("quarantine-release-auction-return-case-bound"),
        admin_fixture!("quarantine-release-auction-recovery-return"),
    ] {
        assert_v2_receipt_schema::<QuarantineReleaseReceipt>(
            decode::<Value>(fixture)["data"].clone(),
            "隔离解除回执",
        );
    }
    assert_v2_receipt_schema::<ConsignmentHoldReceipt>(
        decode::<Value>(admin_fixture!("hold-set-receipt"))["data"].clone(),
        "设置止售回执",
    );
    assert_v2_receipt_schema::<ConsignmentHoldReceipt>(
        decode::<Value>(admin_fixture!("hold-released-receipt"))["data"].clone(),
        "解除止售回执",
    );
    assert_v2_receipt_schema::<ConsignmentDelistReceiptV2>(
        decode::<Value>(admin_fixture!("delist-closed-receipt"))["data"].clone(),
        "未代管下架回执",
    );
    assert_v2_receipt_schema::<ConsignmentDelistReceiptV2>(
        decode::<Value>(admin_fixture!("hold-set-receipt"))["data"].clone(),
        "代管件下架回执",
    );
}

#[test]
fn admin_v2_quarantine_and_hold_fixtures_round_trip() {
    round_trip::<ApiResponse<QuarantineDetail>>(
        "quarantine-active",
        admin_fixture!("quarantine-active"),
    );
    round_trip::<ApiResponse<QuarantineDetail>>(
        "quarantine-released",
        admin_fixture!("quarantine-released"),
    );
    round_trip::<QuarantineRecheckRequest>(
        "quarantine-recheck-request",
        admin_fixture!("quarantine-recheck-request"),
    );
    round_trip::<ApiResponse<QuarantineRecheckReceipt>>(
        "quarantine-recheck-receipt",
        admin_fixture!("quarantine-recheck-receipt"),
    );
    round_trip::<QuarantineReleaseRequest>(
        "quarantine-release-request",
        admin_fixture!("quarantine-release-request"),
    );
    for (name, fixture) in [
        ("quarantine-release-accept", admin_fixture!("quarantine-release-accept")),
        ("quarantine-release-return", admin_fixture!("quarantine-release-return")),
        ("quarantine-release-archive-empty", admin_fixture!("quarantine-release-archive-empty")),
        ("quarantine-release-acquire-return", admin_fixture!("quarantine-release-acquire-return")),
        (
            "quarantine-release-auction-return-case-unbound",
            admin_fixture!("quarantine-release-auction-return-case-unbound"),
        ),
        (
            "quarantine-release-auction-return-case-bound",
            admin_fixture!("quarantine-release-auction-return-case-bound"),
        ),
        (
            "quarantine-release-auction-recovery-return",
            admin_fixture!("quarantine-release-auction-recovery-return"),
        ),
    ] {
        round_trip::<ApiResponse<QuarantineReleaseReceipt>>(name, fixture);
    }
    round_trip::<ApiResponse<ConsignmentHoldReceipt>>(
        "hold-set-receipt",
        admin_fixture!("hold-set-receipt"),
    );
    round_trip::<ApiResponse<ConsignmentHoldReceipt>>(
        "hold-released-receipt",
        admin_fixture!("hold-released-receipt"),
    );
    round_trip::<ApiResponse<ConsignmentDelistReceiptV2>>(
        "delist-closed-receipt",
        admin_fixture!("delist-closed-receipt"),
    );
    round_trip::<ApiResponse<ConsignmentDelistReceiptV2>>(
        "hold-set-receipt",
        admin_fixture!("hold-set-receipt"),
    );
}

#[test]
fn v2_status_requires_exact_world_keys_and_season_shapes() {
    let data = public_status_data();
    let worlds = data["worlds"].clone();
    let main = worlds["main"].clone();
    let isekai = worlds["isekai"].clone();

    let mut extra_world = worlds.clone();
    extra_world["other"] = main.clone();
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", extra_world),
        "worlds 多出第三个键",
    );
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "main", isekai.clone())),
        "main 使用 isekai 能力形状",
    );
    for missing in ["main", "isekai"] {
        assert_rejects::<ConsignmentStatusV2>(
            with_field(&data, "worlds", without_field(&worlds, missing)),
            "worlds 缺少键",
        );
    }

    let mut main_with_time = main.clone();
    main_with_time["cutoff_at"] = json!("2026-12-01T00:00:00Z");
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "main", main_with_time.clone())),
        "main 预停时刻非 null",
    );
    main_with_time["cutoff_at"] = Value::Null;
    main_with_time["season_end_at"] = json!("2026-12-15T00:00:00Z");
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "main", main_with_time)),
        "main 季末时刻非 null",
    );
    let mut main_open = main.clone();
    main_open["season_state"] = json!("open");
    main_open["cutoff_at"] = json!("2026-12-01T00:00:00Z");
    main_open["season_end_at"] = json!("2026-12-15T00:00:00Z");
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "main", main_open)),
        "main 自称有赛季",
    );
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "isekai", main)),
        "isekai 自称无赛季",
    );

    let mut unknown_with_time = isekai;
    unknown_with_time["season_state"] = json!("unknown");
    unknown_with_time["mail_ready"] = json!(false);
    unknown_with_time["purchase_ready"] = json!(false);
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "isekai", unknown_with_time.clone())),
        "isekai 未知季历仍给出时刻",
    );
    unknown_with_time["cutoff_at"] = Value::Null;
    unknown_with_time["season_end_at"] = Value::Null;
    assert!(
        serde_json::from_value::<ConsignmentStatusV2>(with_field(
            &data,
            "worlds",
            with_field(&worlds, "isekai", unknown_with_time.clone()),
        ))
        .is_ok()
    );
    unknown_with_time["mail_ready"] = json!(true);
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "worlds", with_field(&worlds, "isekai", unknown_with_time)),
        "未知季历仍宣告收件",
    );

    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "accepts_mail", json!(false)),
        "汇总收件能力与两世界 OR 不符",
    );
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "purchasable", json!(true)),
        "汇总购买能力与两世界 OR 不符",
    );
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "registration_modes", json!([])),
        "登记就绪但模式为空",
    );
    for field in ["min_price", "max_price"] {
        assert_rejects::<ConsignmentStatusV2>(
            with_field(&data, field, json!(if field == "min_price" { 0 } else { 1_000_000_001 })),
            "价格协议常量",
        );
    }
}

#[test]
fn v2_status_and_listing_keep_future_purchase_capability_typed() {
    let data = public_status_data();
    let worlds = data["worlds"].clone();
    let mut isekai = worlds["isekai"].clone();
    isekai["purchase_ready"] = json!(true);
    let opened = with_field(
        &with_field(&data, "worlds", with_field(&worlds, "isekai", isekai)),
        "purchasable",
        json!(true),
    );
    let status: ConsignmentStatusV2 = serde_json::from_value(opened).unwrap();
    assert!(status.worlds.isekai.purchase_ready);
    assert!(status.purchasable);

    let detail = decode::<ApiResponse<ListingDetailV2>>(public_fixture!("listing-v2-public"));
    let mut summary = serde_json::to_value(detail.data.summary).unwrap();
    summary["purchasable"] = json!(true);
    summary["allowed_actions"] = json!(["purchase"]);
    let summary: ListingSummaryV2 = serde_json::from_value(summary).unwrap();
    assert!(summary.purchasable);
    assert_eq!(summary.allowed_actions, [ListingActionV2::Purchase]);
}

#[test]
fn v2_status_rejects_registration_contradictions() {
    let data = public_status_data();
    assert_rejects::<ConsignmentStatusV2>(
        with_field(&data, "enabled", json!(false)),
        "关闭状态仍可登记",
    );
    let disabled: Value = decode::<Value>(public_fixture!("status-v2-disabled"))["data"].clone();
    assert!(serde_json::from_value::<ConsignmentStatusV2>(disabled.clone()).is_ok());
    for (field, value) in
        [("registration_modes", json!(["direct"])), ("registration_worlds", json!(["main"]))]
    {
        assert_rejects::<ConsignmentStatusV2>(
            with_field(&disabled, field, value),
            "未就绪仍给出登记选项",
        );
    }
}

#[test]
fn v2_deposit_instructions_require_all_active_fields() {
    let none: Value = decode::<Value>(public_fixture!("deposit-instructions-none"))["data"].clone();
    let active = active_instructions();
    assert_eq!(
        serde_json::from_value::<DepositInstructions>(none.clone()).unwrap(),
        DepositInstructions::None {}
    );
    assert_rejects::<DepositInstructions>(
        with_field(&none, "body", json!("C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")),
        "none 携带秘密正文",
    );
    assert_rejects::<DepositInstructions>(
        with_field(&none, "world", json!("main")),
        "none 携带世界",
    );

    for missing in ["world", "bot", "subject", "body", "reservation_id", "expires_at"] {
        assert_rejects::<DepositInstructions>(
            without_field(&active, missing),
            "active 缺少必填字段",
        );
    }
    for (field, value) in [
        ("world", json!("other")),
        ("bot", json!("")),
        ("subject", json!("Consign")),
        ("subject", json!("consign ")),
        ("body", json!("C1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")),
        ("body", json!("C1-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")),
        ("body", json!("c1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")),
        ("body", json!("C1-===========================================")),
        ("reservation_id", json!("00000000-0000-0000-0000-000000000000")),
        ("reservation_id", json!("00000000-0000-4000-8000-00000000030A")),
        ("expires_at", json!("2026-11-27T18:10:00-08:00")),
        ("expires_at", Value::Null),
    ] {
        assert_rejects::<DepositInstructions>(
            with_field(&active, field, value),
            "非法 active 指引字段",
        );
    }
}

#[test]
fn v2_deposit_body_is_never_echoed_in_debug_or_errors() {
    let active = active_instructions();
    let body = active["body"].as_str().unwrap().to_owned();
    let instructions: DepositInstructions = serde_json::from_value(active.clone()).unwrap();
    let printed = format!("{instructions:?}");
    assert!(printed.contains("[REDACTED]"));
    assert!(!printed.contains(&body));
    let wire = serde_json::to_value(&instructions).unwrap();
    assert_eq!(wire["body"], body);

    let fake_key = "0123456789";
    let malformed = format!("C1-{fake_key}{}", "A".repeat(32));
    let error = serde_json::from_value::<DepositInstructions>(with_field(
        &active,
        "body",
        json!(&malformed),
    ))
    .unwrap_err()
    .to_string();
    assert!(!error.contains(&malformed));
    assert!(!error.contains(fake_key));

    let fake_url = "https://example.invalid/equip/42/0123456789";
    let error = serde_json::from_value::<DepositInstructions>(with_field(
        &active,
        "equipment_url",
        json!(fake_url),
    ))
    .unwrap_err()
    .to_string();
    assert!(!error.contains(fake_url));
    assert!(!error.contains("0123456789"));
}

#[test]
fn v2_deposit_reservation_receipt_requires_awaiting_deposit() {
    let receipt: Value =
        decode::<Value>(public_fixture!("deposit-reservation-receipt"))["data"].clone();
    assert!(serde_json::from_value::<DepositReservationReceipt>(receipt.clone()).is_ok());
    for state in ["available", "deposit_accepted", "withdrawal_pending"] {
        assert_rejects::<DepositReservationReceipt>(
            with_field(&receipt, "state", json!(state)),
            "预约回执状态不是 awaiting_deposit",
        );
    }
}

#[test]
fn v2_withdraw_receipt_accepts_managed_and_unmanaged_results() {
    let receipt: Value =
        decode::<Value>(public_fixture!("withdrawal-requested-receipt"))["data"].clone();
    for state in ["deposit_accepted", "withdrawal_pending", "held_off_market"] {
        assert!(
            serde_json::from_value::<WithdrawReceiptV2>(with_field(
                &receipt,
                "state",
                json!(state),
            ))
            .is_ok(),
            "{state} 的撤回意愿回执应被接受"
        );
    }
    for state in ["available", "resolving", "closed"] {
        assert_rejects::<WithdrawReceiptV2>(
            with_field(&receipt, "state", json!(state)),
            "撤回意愿回执状态不属于第 4 期三态",
        );
    }
    let closed: Value = decode::<Value>(public_fixture!("withdraw-closed-receipt"))["data"].clone();
    assert!(serde_json::from_value::<WithdrawReceiptV2>(closed.clone()).is_ok());
    for state in ["available", "awaiting_deposit", "withdrawal_pending"] {
        assert_rejects::<WithdrawReceiptV2>(
            with_field(&closed, "state", json!(state)),
            "未代管撤回回执状态不是 closed",
        );
    }
    assert_rejects::<WithdrawReceiptV2>(
        with_field(&closed, "intent_id", json!("00000000-0000-4000-8000-000000000417")),
        "未代管撤回回执不得携带意愿",
    );
}

#[test]
fn v2_public_listing_cannot_express_private_states_or_actions() {
    let detail: ApiResponse<ListingDetailV2> = decode(public_fixture!("listing-v2-public"));
    let detail_wire = serde_json::to_value(&detail.data).unwrap();
    let summary = detail_wire["summary"].clone();
    for state in ["deposit_accepted", "sale_pending", "held_off_market", "withdrawal_pending"] {
        assert_rejects::<ListingSummaryV2>(
            with_field(&summary, "state", json!(state)),
            "公开模型表达私有状态",
        );
    }
    for action in ["admin_delist", "release_hold", "reprice"] {
        assert_rejects::<ListingSummaryV2>(
            with_field(&summary, "allowed_actions", json!([action])),
            "公开模型表达非购买动作",
        );
    }
    assert_rejects::<ListingSummaryV2>(
        with_field(&summary, "allowed_actions", json!(["purchase", "purchase"])),
        "公开动作重复",
    );
    assert_rejects::<ListingSummaryV2>(
        with_field(&summary, "generation", Value::Null),
        "available 缺处分代",
    );
    let mut awaiting = summary.clone();
    awaiting["state"] = json!("awaiting_deposit");
    awaiting["generation"] = Value::Null;
    assert!(serde_json::from_value::<ListingSummaryV2>(awaiting.clone()).is_ok());
    awaiting["generation"] = json!(1);
    assert_rejects::<ListingSummaryV2>(awaiting, "未代管却携带处分代");

    let mut soulbound = summary.clone();
    soulbound["level"] = json!({"kind": "soulbound"});
    assert_rejects::<ListingSummaryV2>(soulbound, "公开模型展示魂绑装备");
    assert_rejects::<ListingSummaryV2>(
        with_field(&summary, "purchasable", json!(true)),
        "公开挂单可购买但未宣告 purchase",
    );
    assert_rejects::<ListingSummaryV2>(
        with_field(
            &with_field(&summary, "allowed_actions", json!(["purchase"])),
            "purchasable",
            json!(false),
        ),
        "公开挂单宣告 purchase 但不可购买",
    );
    assert_rejects::<ListingSummaryV2>(
        with_field(&summary, "listing_id", json!("00000000-0000-0000-0000-000000000000")),
        "公开挂单零 UUID",
    );
    let equipment = serde_json::to_value(&detail.data.equipment).unwrap();
    assert_rejects::<ListingDetailV2>(
        with_field(
            &detail_wire,
            "equipment",
            with_field(&equipment, "name", json!("Fixture Other Staff")),
        ),
        "详情摘要与快照不一致",
    );
}

#[test]
fn v2_owned_listing_covers_all_states_and_restricts_actions() {
    let views: Vec<ListingViewV2> = decode(public_fixture!("listing-views-v2"));
    assert_eq!(views.len(), 12);
    let kinds = [
        "resolving",
        "lookup_failed",
        "ineligible",
        "displayed",
        "awaiting_deposit",
        "deposit_accepted",
        "available",
        "sale_pending",
        "held_off_market",
        "withdrawal_pending",
        "closed",
        "expired",
    ];
    for (view, expected) in views.iter().zip(kinds) {
        let wire = serde_json::to_value(view).unwrap();
        assert_eq!(wire["kind"], expected);
    }

    let available = owned_available();
    let held = owned_held();
    assert!(serde_json::from_value::<OwnedListingV2>(available.clone()).is_ok());
    assert!(serde_json::from_value::<OwnedListingV2>(held.clone()).is_ok());
    let mut quota_held = held.clone();
    quota_held["view"]["reason"] = json!("quota_blocked");
    quota_held["view"]["hold_id"] = Value::Null;
    quota_held["effective_reason"] = json!("quota_blocked");
    quota_held["allowed_actions"] = json!(["withdraw", "relist"]);
    assert!(serde_json::from_value::<OwnedListingV2>(quota_held).is_ok());
    let mut operations_relist = held.clone();
    operations_relist["allowed_actions"] = json!(["withdraw", "relist"]);
    assert_rejects::<OwnedListingV2>(operations_relist, "运营止售仍公布重列");
    // 本人视图不带 hold_id：运营止售的 hold_id 为空也合法。
    let mut owner_without_hold_id = held.clone();
    owner_without_hold_id["view"]["hold_id"] = Value::Null;
    assert!(serde_json::from_value::<OwnedListingV2>(owner_without_hold_id).is_ok());
    // 只有运营止售（对已接受未取得的件）可以没有处分代，其余原因必须有。
    let mut quota_without_generation = held.clone();
    quota_without_generation["view"]["reason"] = json!("quota_blocked");
    quota_without_generation["effective_reason"] = json!("quota_blocked");
    quota_without_generation["generation"] = Value::Null;
    assert_rejects::<OwnedListingV2>(quota_without_generation, "非运营止售的 held 无处分代");

    for action in [json!(["purchase"]), json!(["relist"]), json!(["reprice", "reprice"])] {
        assert_rejects::<OwnedListingV2>(
            with_field(&available, "allowed_actions", action),
            "本人挂单动作与状态不符",
        );
    }
    assert_rejects::<OwnedListingV2>(
        with_field(&available, "generation", Value::Null),
        "available 缺处分代",
    );
    assert_rejects::<OwnedListingV2>(
        with_field(&available, "effective_state", json!("held_off_market")),
        "有效状态与视图不符",
    );
    assert_rejects::<OwnedListingV2>(
        with_field(&available, "effective_reason", json!("review")),
        "活动视图携带原因",
    );

    for (key, reason) in [
        ("closed", json!("quota_blocked")),
        ("expired", json!("sold")),
        ("held_off_market", json!("sold")),
        ("lookup_failed", json!("not_tradeable")),
        ("ineligible", json!("owner_missing")),
    ] {
        let mut view = if key == "held_off_market" {
            held["view"].clone()
        } else {
            views
                .iter()
                .find(|view| serde_json::to_value(view).unwrap()["kind"] == key)
                .map(|view| serde_json::to_value(view).unwrap())
                .unwrap()
        };
        view["reason"] = reason;
        assert_rejects::<ListingViewV2>(view, "视图原因类别不符");
    }
}

#[test]
fn v2_held_and_closed_dual_reason_views_are_both_legal() {
    assert_eq!(ListingReasonV2::SellerIneligible.terminal_state(), None);
    assert_eq!(ListingReasonV2::ServiceDisabled.terminal_state(), None);
    let held = owned_held();
    for reason in ["seller_ineligible", "service_disabled"] {
        let mut held_wire = held.clone();
        held_wire["view"]["reason"] = json!(reason);
        held_wire["effective_reason"] = json!(reason);
        held_wire["view"]["hold_id"] = Value::Null;
        assert!(
            serde_json::from_value::<OwnedListingV2>(held_wire).is_ok(),
            "{reason} 的代管件应保持 held_off_market"
        );

        let mut closed = held.clone();
        closed["stored_state"] = json!("closed");
        closed["effective_state"] = json!("closed");
        closed["effective_reason"] = json!(reason);
        closed["generation"] = Value::Null;
        closed["allowed_actions"] = json!([]);
        closed["view"] = json!({
            "kind": "closed",
            "reason": reason,
            "equipment": held["view"]["equipment"].clone(),
        });
        assert!(
            serde_json::from_value::<OwnedListingV2>(closed).is_ok(),
            "{reason} 的未代管件应保持 closed"
        );
    }
}

#[test]
fn v2_quarantine_detail_validates_observation_and_release_shapes() {
    let active = quarantine_active();
    let released = quarantine_released();
    let detail: QuarantineDetail = serde_json::from_value(active.clone()).unwrap();
    assert_eq!(detail.observations.len(), 2);
    assert_eq!(detail.state, QuarantineState::Active);
    let released_detail: QuarantineDetail = serde_json::from_value(released.clone()).unwrap();
    assert_eq!(released_detail.state, QuarantineState::Released);
    assert!(released_detail.release.is_some());

    let mut with_release = active.clone();
    with_release["release"] = released["release"].clone();
    assert_rejects::<QuarantineDetail>(with_release, "活动隔离携带解除记录");
    let mut without_release = released.clone();
    without_release["release"] = Value::Null;
    assert_rejects::<QuarantineDetail>(without_release, "已解除隔离缺解除记录");
    assert_rejects::<QuarantineDetail>(
        with_field(&released, "allowed_actions", json!(["recheck"])),
        "已解除隔离仍有动作",
    );
    assert_rejects::<QuarantineDetail>(
        with_field(&active, "allowed_actions", json!(["recheck", "recheck"])),
        "隔离动作重复",
    );
    assert_rejects::<QuarantineDetail>(
        with_field(&active, "allowed_actions", json!(["publish"])),
        "未知隔离动作",
    );
    assert_rejects::<QuarantineDetail>(
        with_field(&active, "external_mid", json!("042")),
        "非规范 MID",
    );
    assert_rejects::<QuarantineDetail>(with_field(&active, "version", json!(0)), "隔离版本为零");
    assert!(
        serde_json::from_value::<QuarantineDetail>(with_field(
            &active,
            "observation_total",
            json!(250),
        ))
        .is_ok(),
        "观察总数大于返回条数时应可解码"
    );
    assert_rejects::<QuarantineDetail>(
        with_field(&active, "observation_total", json!(1)),
        "观察总数小于返回条数",
    );
    assert_rejects::<QuarantineDetail>(
        with_field(&active, "observation_total", json!(-1)),
        "观察总数为负",
    );

    let mut count_mismatch = active.clone();
    count_mismatch["observations"][0]["attachment_count"] = json!(2);
    assert_rejects::<QuarantineDetail>(count_mismatch, "附件数量与清单不符");
    let mut duplicate_ordinal = active.clone();
    duplicate_ordinal["observations"][1]["items"][1]["ordinal"] = json!(1);
    assert_rejects::<QuarantineDetail>(duplicate_ordinal, "观察项序号重复");
    let mut negative_credits = active.clone();
    negative_credits["observations"][0]["credits"] = json!(-1);
    assert_rejects::<QuarantineDetail>(negative_credits, "观察金额为负");
    let mut uppercase_digest = active.clone();
    uppercase_digest["observations"][0]["digest"] = json!("A".repeat(64));
    assert_rejects::<QuarantineDetail>(uppercase_digest, "摘要为大写");
    let mut empty_parser = active.clone();
    empty_parser["observations"][0]["parser_version"] = json!("");
    assert_rejects::<QuarantineDetail>(empty_parser, "解析器版本为空");
    let mut unknown_with_amount = active.clone();
    unknown_with_amount["observations"][1]["items"][1]["amount"] = json!(0);
    assert_rejects::<QuarantineDetail>(unknown_with_amount, "未知附件携带金额");
    let mut missing_credits = active;
    missing_credits["observations"][0].as_object_mut().unwrap().remove("credits");
    assert_rejects::<QuarantineDetail>(missing_credits, "缺少可空 credits 键");
    let mut same_decision = released;
    same_decision["release"]["decision_id"] = same_decision["first_decision_id"].clone();
    assert_rejects::<QuarantineDetail>(same_decision, "最终决定等于首次隔离决定");
}

#[test]
fn v2_quarantine_release_branches_are_mutually_exclusive() {
    let _accept = decode::<ApiResponse<QuarantineReleaseReceipt>>(admin_fixture!(
        "quarantine-release-accept"
    ));
    let unbound = decode::<ApiResponse<QuarantineReleaseReceipt>>(admin_fixture!(
        "quarantine-release-auction-return-case-unbound"
    ));
    let bound = decode::<ApiResponse<QuarantineReleaseReceipt>>(admin_fixture!(
        "quarantine-release-auction-return-case-bound"
    ));
    let QuarantineReleaseReceipt::AuctionReturnCase { obligation_id: unbound_obligation, .. } =
        &unbound.data
    else {
        panic!("无绑定案件分支");
    };
    let QuarantineReleaseReceipt::AuctionReturnCase { obligation_id: bound_obligation, .. } =
        &bound.data
    else {
        panic!("有绑定案件分支");
    };
    assert!(unbound_obligation.is_none());
    assert!(bound_obligation.is_some());

    let accept_wire: Value = decode(admin_fixture!("quarantine-release-accept"));
    let mut accept_with_auction = accept_wire.clone();
    accept_with_auction["data"]["return_mail_id"] = json!("00000000-0000-4000-8000-000000000408");
    assert_rejects::<QuarantineReleaseReceipt>(
        accept_with_auction["data"].clone(),
        "accept 分支混入拍卖字段",
    );
    let case_wire: Value = decode(admin_fixture!("quarantine-release-auction-return-case-unbound"));
    let mut case_with_intake = case_wire.clone();
    case_with_intake["data"]["return_intake_id"] = json!("00000000-0000-4000-8000-000000000413");
    assert_rejects::<QuarantineReleaseReceipt>(
        case_with_intake["data"].clone(),
        "拍卖案件分支混入寄售 intake",
    );
    let recovery_wire: Value = decode(admin_fixture!("quarantine-release-auction-recovery-return"));
    let mut recovery_with_take = recovery_wire.clone();
    recovery_with_take["data"]["take_work_id"] = json!("00000000-0000-4000-8000-000000000414");
    assert_rejects::<QuarantineReleaseReceipt>(
        recovery_with_take["data"].clone(),
        "恢复分支混入 Take 字段",
    );

    let mut missing_listings = accept_wire.clone();
    missing_listings["data"].as_object_mut().unwrap().remove("listing_changes");
    assert_rejects::<QuarantineReleaseReceipt>(missing_listings["data"].clone(), "accept 缺逐件 L");
    let mut empty_listings = accept_wire.clone();
    empty_listings["data"]["listing_changes"] = json!([]);
    assert_rejects::<QuarantineReleaseReceipt>(
        empty_listings["data"].clone(),
        "accept 逐件 L 为空",
    );
    let mut wrong_listing_state = accept_wire.clone();
    wrong_listing_state["data"]["listing_changes"][0]["state"] = json!("available");
    assert_rejects::<QuarantineReleaseReceipt>(
        wrong_listing_state["data"].clone(),
        "accept 逐件 L 非 deposit_accepted",
    );
    let mut duplicate_listing = accept_wire.clone();
    duplicate_listing["data"]["listing_changes"][1]["listing_id"] =
        duplicate_listing["data"]["listing_changes"][0]["listing_id"].clone();
    assert_rejects::<QuarantineReleaseReceipt>(
        duplicate_listing["data"].clone(),
        "accept 逐件 L 重复",
    );

    let acquire: Value = decode(admin_fixture!("quarantine-release-acquire-return"));
    for missing in ["return_intake_id", "original_round", "take_work_id", "take_delivery_id"] {
        assert_rejects::<QuarantineReleaseReceipt>(
            without_field(&acquire["data"], missing),
            "acquire_return 缺字段",
        );
    }
    for missing in ["obligation_id", "return_mail_id", "return_evidence_id"] {
        assert_rejects::<QuarantineReleaseReceipt>(
            without_field(&case_wire["data"], missing),
            "auction_return_case 缺字段",
        );
    }
    for missing in ["obligation_id", "return_mail_id", "recovery_id", "recovery_handoff_id"] {
        assert_rejects::<QuarantineReleaseReceipt>(
            without_field(&recovery_wire["data"], missing),
            "auction_recovery_return 缺字段",
        );
    }
    for field in ["obligation_id", "return_mail_id", "recovery_id", "recovery_handoff_id"] {
        assert_rejects::<QuarantineReleaseReceipt>(
            with_field(
                &recovery_wire["data"],
                field,
                json!("00000000-0000-0000-0000-000000000000"),
            ),
            "auction_recovery_return 引用零 UUID",
        );
    }

    let return_wire: Value = decode(admin_fixture!("quarantine-release-return"));
    assert_rejects::<QuarantineReleaseReceipt>(
        with_field(
            &return_wire["data"],
            "obligation_id",
            json!("00000000-0000-4000-8000-000000000410"),
        ),
        "return 分支混入拍卖字段",
    );
    assert_rejects::<QuarantineReleaseReceipt>(
        with_field(&accept_wire["data"], "disposition", json!("hold_set")),
        "解除回执处置不符",
    );
    assert_rejects::<QuarantineReleaseReceipt>(
        with_field(
            &accept_wire["data"],
            "quarantine_id",
            json!("00000000-0000-0000-0000-000000000000"),
        ),
        "解除回执零 UUID",
    );

    let release_request: Value = decode(admin_fixture!("quarantine-release-request"));
    assert_rejects::<QuarantineReleaseRequest>(
        with_field(&release_request, "proposed_kind", json!("force")),
        "未知解除种类",
    );
    assert_rejects::<QuarantineReleaseRequest>(
        with_field(&release_request, "force", json!(true)),
        "跳过证明的 force",
    );
}

#[test]
fn v2_hold_receipts_match_operation_disposition_and_state() {
    let hold_set: ConsignmentHoldReceipt =
        decode::<ApiResponse<ConsignmentHoldReceipt>>(admin_fixture!("hold-set-receipt")).data;
    assert_eq!(hold_set.operation, ConsignmentOperationV2::AdminDelist);
    assert_eq!(hold_set.disposition, ConsignmentDispositionV2::HoldSet);
    assert_eq!(hold_set.state, ListingStateV2::HeldOffMarket);
    let hold_released: ConsignmentHoldReceipt =
        decode::<ApiResponse<ConsignmentHoldReceipt>>(admin_fixture!("hold-released-receipt")).data;
    assert_eq!(hold_released.operation, ConsignmentOperationV2::ReleaseHold);
    assert_eq!(hold_released.disposition, ConsignmentDispositionV2::HoldReleased);
    assert_eq!(hold_released.state, ListingStateV2::HeldOffMarket);
    let wire = serde_json::to_value(hold_set).unwrap();
    for (field, value) in [
        ("state", json!("available")),
        ("disposition", json!("hold_released")),
        ("hold_id", json!("00000000-0000-0000-0000-000000000000")),
    ] {
        assert_rejects::<ConsignmentHoldReceipt>(
            with_field(&wire, field, value),
            "止售回执字段不符",
        );
    }
    let released_wire = serde_json::to_value(hold_released).unwrap();
    assert_rejects::<ConsignmentHoldReceipt>(
        with_field(&released_wire, "state", json!("available")),
        "解除止售必须保持 held_off_market",
    );
}

#[test]
fn v2_delist_receipt_covers_closed_and_hold_set() {
    let closed: ConsignmentDelistReceiptV2 =
        decode::<ApiResponse<ConsignmentDelistReceiptV2>>(admin_fixture!("delist-closed-receipt"))
            .data;
    let ConsignmentDelistReceiptV2::Closed { operation, state, .. } = closed else {
        panic!("未代管下架回执必须是 closed 分支");
    };
    assert_eq!(operation, ConsignmentOperationV2::AdminDelist);
    assert_eq!(state, ListingStateV2::Closed);
    let hold: ConsignmentDelistReceiptV2 =
        decode::<ApiResponse<ConsignmentDelistReceiptV2>>(admin_fixture!("hold-set-receipt")).data;
    let ConsignmentDelistReceiptV2::HoldSet { operation, state, .. } = hold else {
        panic!("代管件下架回执必须是 hold_set 分支");
    };
    assert_eq!(operation, ConsignmentOperationV2::AdminDelist);
    assert_eq!(state, ListingStateV2::HeldOffMarket);

    let closed_wire: Value = decode(admin_fixture!("delist-closed-receipt"));
    let closed_wire = closed_wire["data"].clone();
    for (field, value) in [
        ("state", json!("available")),
        ("disposition", json!("hold_set")),
        ("hold_id", json!("00000000-0000-4000-8000-000000000416")),
        ("schema_version", json!(1)),
    ] {
        assert_rejects::<ConsignmentDelistReceiptV2>(
            with_field(&closed_wire, field, value),
            "未代管下架回执字段不符",
        );
    }
    let hold_wire: Value = decode(admin_fixture!("hold-set-receipt"));
    let hold_wire = hold_wire["data"].clone();
    for (field, value) in [
        ("state", json!("closed")),
        ("hold_id", json!("00000000-0000-0000-0000-000000000000")),
        ("operation", json!("release_hold")),
    ] {
        assert_rejects::<ConsignmentDelistReceiptV2>(
            with_field(&hold_wire, field, value),
            "代管件下架回执字段不符",
        );
    }
}

#[test]
fn v2_requests_enforce_uuidv4_and_version_ranges() {
    let reservation: Value = decode(public_fixture!("deposit-reservation-request"));
    let recheck: Value = decode(admin_fixture!("quarantine-recheck-request"));
    let release: Value = decode(admin_fixture!("quarantine-release-request"));
    let withdraw: Value = decode(public_fixture!("withdrawal-request-v2"));
    for (name, request) in [
        ("reservation", reservation.clone()),
        ("recheck", recheck.clone()),
        ("release", release.clone()),
        ("withdraw", withdraw.clone()),
    ] {
        for uuid in [
            "00000000-0000-5000-8000-000000000001",
            "00000000-0000-0000-8000-000000000001",
            "00000000-0000-4000-8000-00000000000A",
            "00000000-0000-0000-0000-000000000000",
        ] {
            let changed = with_field(&request, "command_id", json!(uuid));
            let scenario = format!("{name} 非法命令 ID {uuid}");
            if name == "reservation" {
                assert_rejects::<IssueDepositReservation>(changed, &scenario);
            } else if name == "recheck" {
                assert_rejects::<QuarantineRecheckRequest>(changed, &scenario);
            } else if name == "withdraw" {
                assert_rejects::<WithdrawListingV2>(changed, &scenario);
            } else {
                assert_rejects::<QuarantineReleaseRequest>(changed, &scenario);
            }
        }
    }

    for version in [json!(0), json!(-1), json!(1.5), json!("1"), json!(9_007_199_254_740_992_i64)] {
        assert_rejects::<IssueDepositReservation>(
            with_field(&reservation, "expected_version", version.clone()),
            "非法预约预期版本",
        );
        assert_rejects::<QuarantineRecheckRequest>(
            with_field(&recheck, "expected_version", version.clone()),
            "非法重查预期版本",
        );
        assert_rejects::<QuarantineReleaseRequest>(
            with_field(&release, "expected_version", version.clone()),
            "非法解除预期版本",
        );
        assert_rejects::<WithdrawListingV2>(
            with_field(&withdraw, "expected_version", version),
            "非法撤回预期版本",
        );
    }
    for missing in ["command_id", "expected_version", "observation_id", "proposed_kind"] {
        assert_rejects::<QuarantineReleaseRequest>(
            without_field(&release, missing),
            "解除请求缺字段",
        );
    }
}

#[test]
fn v2_rejection_matrix_matches_shared_fixture() {
    let matrix: Vec<Value> = decode(public_fixture!("write-rejections-v2"));
    assert_eq!(matrix.len(), 21);
    for entry in matrix {
        let code: WriteRejectionCodeV2 = serde_json::from_value(entry["code"].clone()).unwrap();
        assert_eq!(json!(code), entry["code"]);
        assert_eq!(code.http_status(), entry["http_status"].as_i64().unwrap());
        assert_eq!(code.is_outcome_unknown(), entry["outcome_unknown"].as_bool().unwrap());
        assert_eq!(
            code.is_trusted_before_write(),
            entry["trusted_before_write"].as_bool().unwrap()
        );
    }
    assert_rejects::<WriteRejectionCodeV2>(json!("replay_maybe"), "未知拒绝码");
}

#[test]
fn v2_enum_values_match_shared_fixture() {
    let fixture: Value = decode(public_fixture!("enums-v2"));
    macro_rules! check_values {
        ($field:literal, $ty:ty) => {
            for value in fixture[$field].as_array().unwrap() {
                let decoded = serde_json::from_value::<$ty>(value.clone())
                    .unwrap_or_else(|_| panic!("{} 中的 {} 无法解码", $field, value));
                assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
            }
        };
    }
    check_values!("season_states", SeasonStateV2);
    check_values!("listing_states", ListingStateV2);
    check_values!("public_listing_states", PublicListingStateV2);
    check_values!("listing_reasons", ListingReasonV2);
    check_values!("listing_actions", ListingActionV2);
    check_values!("quarantine_states", QuarantineState);
    check_values!("quarantine_reasons", QuarantineReason);
    assert_eq!(fixture["quarantine_reasons"].as_array().unwrap().len(), 12);
    for retired in [
        "duplicate_external_equipment_id",
        "canonical_equipment_id_ambiguous",
        "manifest_conflict",
        "amount_unclear",
        "return_capability_unproven",
        "source_unresolved",
    ] {
        assert_rejects::<QuarantineReason>(json!(retired), "已替换的旧隔离原因码");
    }
    check_values!("quarantine_actions", QuarantineAction);
    check_values!("observation_item_kinds", QuarantineObservationItemKind);
    check_values!("decision_kinds", QuarantineDecisionKind);
    check_values!("operations", ConsignmentOperationV2);
    check_values!("dispositions", ConsignmentDispositionV2);
    check_values!("rejection_codes", WriteRejectionCodeV2);
    check_values!("request_error_codes", RequestErrorCodeV2);
    assert_eq!(
        fixture["request_error_codes"].as_array().unwrap().len(),
        9,
        "请求错误闭集必须覆盖 4A api_error 的 400 码与 404 command_not_found"
    );
    for value in fixture["hold_reasons"].as_array().unwrap() {
        let reason: ListingReasonV2 = serde_json::from_value(value.clone()).unwrap();
        assert!(reason.is_hold_reason(), "{} 不是 held_off_market 原因", value);
    }
}

#[test]
fn v1_status_remains_closed_and_unchanged() {
    let v1: ApiResponse<ConsignmentStatus> =
        decode(include_str!("fixtures/public/consignment-status.json"));
    assert!(!v1.data.accepts_mail);
    assert!(!v1.data.purchasable);
    let wire = serde_json::to_value(v1.data).unwrap();
    for (field, value) in [("accepts_mail", json!(true)), ("purchasable", json!(true))] {
        assert_rejects::<ConsignmentStatus>(with_field(&wire, field, value), "v1 能力字段被放宽");
    }
}

#[test]
fn v2_public_serialization_keeps_the_safe_field_whitelist() {
    fn assert_no_forbidden(value: &Value) {
        const FORBIDDEN: &[&str] = &[
            "equipment_url",
            "equipment_key",
            "key",
            "access_key",
            "seller_user_id",
            "raw_body",
            "raw_response",
            "reservation_code",
            "token",
        ];
        match value {
            Value::Object(object) => {
                for (field, value) in object {
                    assert!(
                        !FORBIDDEN.contains(&field.as_str()),
                        "v2 公开序列化带出私有字段 {field}"
                    );
                    assert_no_forbidden(value);
                }
            }
            Value::Array(items) => items.iter().for_each(assert_no_forbidden),
            _ => {}
        }
    }

    let detail: ApiResponse<ListingDetailV2> = decode(public_fixture!("listing-v2-public"));
    let owned: ApiResponse<OwnedListingV2> = decode(public_fixture!("listing-v2-owned"));
    let detail = serde_json::to_value(detail.data).unwrap();
    let owned = serde_json::to_value(owned.data).unwrap();
    assert_no_forbidden(&detail);
    assert_no_forbidden(&owned);

    let active = active_instructions();
    let instructions: DepositInstructions = serde_json::from_value(active).unwrap();
    let serialized = serde_json::to_value(&instructions).unwrap();
    let mut actual: Vec<&str> =
        serialized.as_object().unwrap().keys().map(String::as_str).collect();
    actual.sort_unstable();
    let mut expected = ["state", "world", "bot", "subject", "body", "reservation_id", "expires_at"];
    expected.sort_unstable();
    assert_eq!(actual, expected);

    let quarantine = serde_json::to_value(
        decode::<ApiResponse<QuarantineDetail>>(admin_fixture!("quarantine-active")).data,
    )
    .unwrap();
    assert_no_forbidden(&quarantine);
}
