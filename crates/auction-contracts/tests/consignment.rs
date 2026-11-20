//! 寄售共用 JSON 夹具的往返、边界和公开字段白名单。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::AdminDelistListing;
use auction_contracts::AdminListing;
use auction_contracts::AdminListingDetail;
use auction_contracts::ApiResponse;
use auction_contracts::CommandReceipt;
use auction_contracts::ConsignmentStatus;
use auction_contracts::EquipmentLevel;
use auction_contracts::EquipmentStat;
use auction_contracts::EquipmentView;
use auction_contracts::EquipmentViewSource;
use auction_contracts::ListingAction;
use auction_contracts::ListingDetail;
use auction_contracts::ListingDisposition;
use auction_contracts::ListingFeeBps;
use auction_contracts::ListingMode;
use auction_contracts::ListingOperation;
use auction_contracts::ListingPage;
use auction_contracts::ListingPrice;
use auction_contracts::ListingPricingVersion;
use auction_contracts::ListingReason;
use auction_contracts::ListingState;
use auction_contracts::ListingSummary;
use auction_contracts::ListingVersion;
use auction_contracts::ListingView;
use auction_contracts::ListingWorld;
use auction_contracts::LookupProgress;
use auction_contracts::LookupReason;
use auction_contracts::OwnedListing;
use auction_contracts::PublicListingState;
use auction_contracts::RegisterListing;
use auction_contracts::RepriceListing;
use auction_contracts::StoredCommandCode;
use auction_contracts::StoredCommandReceipt;
use auction_contracts::StoredCommandResult;
use auction_contracts::WithdrawListing;
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
    // 请求夹具包含假装备链接，解码与往返的失败信息统一省略正文。
    serde_json::from_str(fixture).unwrap_or_else(|error| {
        panic!("寄售夹具解码失败（{:?}，第 {} 行）", error.classify(), error.line())
    })
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str, fixture: &str) {
    let expected: Value = decode(fixture);
    let decoded: T = decode(fixture);
    let actual = serde_json::to_value(decoded).expect("寄售 DTO 应可序列化");
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

#[test]
fn public_read_fixtures_round_trip() {
    for (name, fixture) in [
        ("status", public_fixture!("status")),
        ("status-disabled", public_fixture!("status-disabled")),
        ("status-unavailable", public_fixture!("status-unavailable")),
    ] {
        round_trip::<ApiResponse<ConsignmentStatus>>(name, fixture);
    }
    round_trip::<ApiResponse<ListingPage<ListingSummary>>>("listings", public_fixture!("listings"));
    round_trip::<ApiResponse<ListingDetail>>("listing-detail", public_fixture!("listing-detail"));
    round_trip::<ApiResponse<ListingDetail>>(
        "listing-detail-awaiting-deposit",
        public_fixture!("listing-detail-awaiting-deposit"),
    );
    round_trip::<ApiResponse<ListingPage<OwnedListing>>>(
        "owned-listings",
        public_fixture!("owned-listings"),
    );
    round_trip::<ApiResponse<OwnedListing>>("owned-listing", public_fixture!("owned-listing"));
    round_trip::<ApiResponse<OwnedListing>>(
        "owned-listing-owner-mismatch",
        public_fixture!("owned-listing-owner-mismatch"),
    );
}

#[test]
fn public_command_fixtures_round_trip() {
    round_trip::<RegisterListing>("register-request", public_fixture!("register-request"));
    round_trip::<RepriceListing>("reprice-request", public_fixture!("reprice-request"));
    round_trip::<WithdrawListing>("withdraw-request", public_fixture!("withdraw-request"));
    for (name, fixture) in [
        ("register-receipt", public_fixture!("register-receipt")),
        ("reprice-receipt", public_fixture!("reprice-receipt")),
        ("unchanged-receipt", public_fixture!("unchanged-receipt")),
        ("withdraw-receipt", public_fixture!("withdraw-receipt")),
    ] {
        round_trip::<ApiResponse<CommandReceipt>>(name, fixture);
    }
    round_trip::<ApiResponse<StoredCommandResult>>(
        "command-result",
        public_fixture!("command-result"),
    );
    round_trip::<ApiResponse<StoredCommandResult>>(
        "command-rejected",
        public_fixture!("command-rejected"),
    );
}

#[test]
fn admin_fixtures_round_trip() {
    round_trip::<ApiResponse<ListingPage<AdminListing>>>(
        "admin/listings",
        admin_fixture!("listings"),
    );
    round_trip::<ApiResponse<AdminListingDetail>>(
        "admin/listing-detail",
        admin_fixture!("listing-detail"),
    );
    round_trip::<AdminDelistListing>("admin/delist-request", admin_fixture!("delist-request"));
    round_trip::<ApiResponse<CommandReceipt>>(
        "admin/delist-receipt",
        admin_fixture!("delist-receipt"),
    );
}

#[test]
fn accepted_registration_resolves_without_equipment_snapshot() {
    let registration: ApiResponse<CommandReceipt> = decode(public_fixture!("register-receipt"));
    let owned: ApiResponse<OwnedListing> = decode(public_fixture!("owned-listing"));
    assert_eq!(registration.code, "accepted");
    assert_eq!(registration.data.disposition, ListingDisposition::Registered);
    assert_eq!(registration.data.listing_id, Some(owned.data.listing_id));
    assert_eq!(registration.data.listing_version, Some(owned.data.version));
    assert_eq!(owned.data.stored_state, ListingState::Resolving);
    assert_eq!(owned.data.effective_state, ListingState::Resolving);
    assert_eq!(owned.data.allowed_actions, [ListingAction::Reprice, ListingAction::Withdraw]);
    let ListingView::Resolving { progress } = &owned.data.view else {
        panic!("首次读取应为解析中");
    };
    assert_eq!((progress.attempts, progress.max_attempts, progress.last_reason), (0, 6, None));
    let wire = serde_json::to_value(&owned.data.view).unwrap();
    assert!(wire.get("equipment").is_none());
    assert_rejects::<ListingView>(with_field(&wire, "equipment", Value::Null), "解析中携带快照键");
}

#[test]
fn owned_views_preserve_rejection_and_effective_closure() {
    let page: ApiResponse<ListingPage<OwnedListing>> = decode(public_fixture!("owned-listings"));
    let items = &page.data.items;
    assert!(matches!(
        items[0].view,
        ListingView::LookupFailed {
            reason: ListingReason::LookupExhausted,
            last_lookup_reason: Some(LookupReason::SourceTimeout)
        }
    ));
    let ListingView::Ineligible { equipment, reason } = &items[1].view else {
        panic!("已解析的拒挂结果应带安全快照");
    };
    assert_eq!(*reason, ListingReason::NotTradeable);
    assert_eq!(equipment.level, EquipmentLevel::Soulbound {});
    assert!(!equipment.tradeable);
    assert!(matches!(&items[2].view, ListingView::Displayed { .. }));
    assert!(matches!(&items[3].view, ListingView::AwaitingDeposit { accepts_mail: false, .. }));
    assert!(matches!(
        &items[4].view,
        ListingView::Closed { reason: ListingReason::AuctionLotCreated, equipment: Some(_) }
    ));
    assert_eq!(items[4].stored_state, ListingState::Displayed);
    assert_eq!(items[4].effective_state, ListingState::Closed);
    assert_eq!(items[4].version.get(), 2);
    assert!(items[4].allowed_actions.is_empty());
    assert!(matches!(
        items[5].view,
        ListingView::Expired { reason: ListingReason::LookupDeadline, equipment: None }
    ));
    let wire = serde_json::to_value(&items[4]).unwrap();
    assert_rejects::<OwnedListing>(
        with_field(&wire, "effective_state", json!("displayed")),
        "有效态与视图不符",
    );
    assert_rejects::<OwnedListing>(
        with_field(&wire, "effective_reason", Value::Null),
        "终态缺原因",
    );
    assert_rejects::<OwnedListing>(
        with_field(&wire, "allowed_actions", json!(["withdraw"])),
        "有效终态给出卖家动作",
    );
}

#[test]
fn public_serialization_matches_the_field_whitelist() {
    fn assert_keys(object: &Value, fields: &[&str]) {
        let keys = object.as_object().unwrap();
        assert_eq!(keys.len(), fields.len());
        assert!(fields.iter().all(|field| keys.contains_key(*field)));
    }

    fn assert_no_private_keys(value: &Value) {
        const FORBIDDEN: &[&str] = &[
            "equipment_url",
            "game_equipment_url",
            "equipment_key",
            "key",
            "seller_user_id",
            "owner",
            "owner_name",
            "raw_response",
            "response_body",
            "raw_body",
        ];
        match value {
            Value::Object(object) => {
                for (field, value) in object {
                    assert!(
                        !FORBIDDEN.contains(&field.as_str()),
                        "公开 DTO 序列化带出私有字段 {field}",
                    );
                    assert_no_private_keys(value);
                }
            }
            Value::Array(items) => items.iter().for_each(assert_no_private_keys),
            _ => {}
        }
    }

    let detail: ApiResponse<ListingDetail> = decode(public_fixture!("listing-detail"));
    let page: ApiResponse<ListingPage<ListingSummary>> = decode(public_fixture!("listings"));
    let status: ApiResponse<ConsignmentStatus> = decode(public_fixture!("status"));
    let detail = serde_json::to_value(detail.data).unwrap();
    let page = serde_json::to_value(page.data).unwrap();
    assert_keys(&detail, &["summary", "equipment"]);
    assert_keys(&page, &["items", "total", "page", "page_size"]);
    assert_keys(
        &detail["summary"],
        &[
            "listing_id",
            "version",
            "mode",
            "state",
            "world_id",
            "external_equipment_id",
            "name",
            "category",
            "level",
            "price",
            "seller_name",
            "updated_at",
            "expires_at",
            "purchasable",
        ],
    );
    let equipment = &detail["equipment"];
    assert_keys(
        equipment,
        &[
            "name",
            "alt_name",
            "category",
            "level",
            "tradeable",
            "stat_groups",
            "source",
            "observed_at",
        ],
    );
    assert_keys(&equipment["stat_groups"][0], &["name", "stats"]);
    assert_keys(&equipment["stat_groups"][0]["stats"][0], &["name", "value", "base"]);
    for projection in [&detail, &page, &serde_json::to_value(status.data).unwrap()] {
        assert_no_private_keys(projection);
    }
    assert_rejects::<ListingSummary>(
        with_field(&detail["summary"], "seller_user_id", Value::Null),
        "公开摘要附带内部 ID",
    );
    assert_rejects::<EquipmentView>(
        with_field(equipment, "equipment_url", Value::Null),
        "快照附带装备链接",
    );
}

#[test]
fn missing_required_fields_are_rejected() {
    let request: Value = decode(public_fixture!("register-request"));
    assert_rejects::<RegisterListing>(without_field(&request, "price"), "登记缺少标价");
    let listing: Value = decode(public_fixture!("listing-detail"));
    assert_rejects::<ListingSummary>(
        without_field(&listing["data"]["summary"], "seller_name"),
        "摘要缺卖家显示名",
    );
    let admin: Value = decode(admin_fixture!("listing-detail"));
    assert_rejects::<AdminListingDetail>(
        without_field(&admin["data"], "conflicts"),
        "Admin 详情缺碰撞列表",
    );
}

#[test]
fn nullable_keys_must_still_appear() {
    let detail: Value = decode(public_fixture!("listing-detail"));
    let equipment = &detail["data"]["equipment"];
    assert!(serde_json::from_value::<EquipmentView>(equipment.clone()).unwrap().alt_name.is_none());
    assert_rejects::<EquipmentView>(without_field(equipment, "alt_name"), "缺少 alt_name");
    let stat = &equipment["stat_groups"][0]["stats"][1];
    assert!(serde_json::from_value::<EquipmentStat>(stat.clone()).unwrap().base.is_none());
    assert_rejects::<EquipmentStat>(without_field(stat, "base"), "缺少 base");

    let own: Value = decode(public_fixture!("owned-listing"));
    assert_rejects::<OwnedListing>(
        without_field(&own["data"], "effective_reason"),
        "缺少 effective_reason",
    );
    let items: Value = decode(public_fixture!("owned-listings"));
    let expired = &items["data"]["items"][5]["view"];
    assert_rejects::<ListingView>(without_field(expired, "equipment"), "到期视图缺少 equipment");
    let failed = &items["data"]["items"][0]["view"];
    let nullable = with_field(failed, "last_lookup_reason", Value::Null);
    assert!(matches!(
        serde_json::from_value::<ListingView>(nullable).unwrap(),
        ListingView::LookupFailed { last_lookup_reason: None, .. }
    ));
    assert_rejects::<ListingView>(
        without_field(failed, "last_lookup_reason"),
        "解析失败缺少 last_lookup_reason",
    );

    let rejected: ApiResponse<StoredCommandResult> = decode(public_fixture!("command-rejected"));
    assert_eq!(rejected.data.receipt.listing_id, None);
    assert_eq!(rejected.data.receipt.listing_version, None);
    assert_eq!(rejected.data.receipt.state, None);
    assert_eq!(rejected.data.receipt.price, None);
    let receipt = serde_json::to_value(rejected.data.receipt).unwrap();
    for field in ["listing_id", "listing_version", "state", "price"] {
        assert_rejects::<StoredCommandReceipt>(
            without_field(&receipt, field),
            "拒绝回执缺少可空键",
        );
    }
    let admin: Value = decode(admin_fixture!("listing-detail"));
    assert_rejects::<AdminListing>(
        without_field(&admin["data"]["record"], "admin_delist_reason"),
        "缺少 admin_delist_reason",
    );
    let request: Value = decode(public_fixture!("register-request"));
    assert_rejects::<RegisterListing>(with_field(&request, "price", Value::Null), "标价为 null");
}

#[test]
fn unknown_enum_values_and_wrong_json_shapes_are_rejected() {
    let request: Value = decode(public_fixture!("register-request"));
    assert_eq!(
        decode::<RegisterListing>(public_fixture!("register-request")).mode,
        ListingMode::Consigned
    );
    assert_rejects::<RegisterListing>(
        with_field(&request, "mode", json!("registered_only")),
        "未知模式",
    );
    assert_rejects::<RegisterListing>(
        with_field(&request, "equipment_url", json!(42)),
        "装备链接为数字",
    );
    assert_rejects::<RegisterListing>(
        with_field(&request, "actor", Value::Null),
        "客户端指定 actor",
    );

    let detail: Value = decode(public_fixture!("listing-detail"));
    let summary = &detail["data"]["summary"];
    assert_rejects::<ListingSummary>(
        with_field(summary, "state", json!("resolving")),
        "目录泄露私有状态",
    );
    assert_rejects::<ListingSummary>(with_field(summary, "world_id", json!("other")), "未知世界");
    let equipment = &detail["data"]["equipment"];
    assert_rejects::<EquipmentView>(
        with_field(equipment, "source", json!("unknown")),
        "未知快照来源",
    );
    assert_rejects::<EquipmentLevel>(json!({"kind": "unknown"}), "未知装备等级");
    assert_rejects::<EquipmentLevel>(
        json!({"kind": "unassigned", "value": 42}),
        "未分配等级附带值",
    );
    assert_rejects::<EquipmentLevel>(json!(["assigned", 42]), "等级以数组代替对象");

    let owned: Value = decode(public_fixture!("owned-listing"));
    assert_rejects::<OwnedListing>(
        with_field(&owned["data"], "allowed_actions", json!(["relist"])),
        "未知卖家动作",
    );
    assert_rejects::<ListingView>(json!({"kind": "registered_only"}), "未知视图");
    assert_rejects::<ListingReason>(json!("upstream_raw_failure"), "未知原因");
    assert_rejects::<ListingOperation>(json!("auto_close"), "内部命令出现在 HTTP 回执");
    assert_rejects::<ListingDisposition>(json!("applied"), "未知回执处置");

    let command_id = "00000000-0000-4000-8000-000000000065";
    assert_rejects::<RepriceListing>(json!([command_id, 1, 42]), "改价命令以数组代替对象");
    assert_rejects::<EquipmentStat>(json!(["Fixture weight", 1.5, null]), "属性以数组代替对象");
    let duplicate =
        format!(r#"{{"command_id":"{command_id}","expected_version":1,"price":10,"price":20}}"#);
    assert!(serde_json::from_str::<RepriceListing>(&duplicate).is_err());
}

#[test]
fn price_fee_and_version_boundaries_are_strict() {
    let register: Value = decode(public_fixture!("register-request"));
    let reprice: Value = decode(public_fixture!("reprice-request"));
    for price in [1, 1_000_000_000] {
        assert_eq!(
            serde_json::from_value::<RegisterListing>(with_field(&register, "price", json!(price)))
                .unwrap()
                .price
                .get(),
            price
        );
        assert_eq!(
            serde_json::from_value::<RepriceListing>(with_field(&reprice, "price", json!(price)))
                .unwrap()
                .price
                .get(),
            price
        );
    }
    for price in [
        json!(0),
        json!(-1),
        json!(1_000_000_001),
        json!(u64::MAX),
        json!("101"),
        json!(101.0),
        json!(1.25),
    ] {
        assert_rejects::<RegisterListing>(
            with_field(&register, "price", price.clone()),
            "非法登记标价",
        );
        assert_rejects::<RepriceListing>(with_field(&reprice, "price", price), "非法改价标价");
    }
    let exponent =
        r#"{"command_id":"00000000-0000-4000-8000-000000000065","expected_version":1,"price":1e2}"#;
    assert!(serde_json::from_str::<RepriceListing>(exponent).is_err());
    for fee in [0, 9999] {
        assert_eq!(
            serde_json::from_value::<RegisterListing>(with_field(
                &register,
                "expected_fee_bps",
                json!(fee)
            ))
            .unwrap()
            .expected_fee_bps
            .get(),
            fee
        );
    }
    for fee in [json!(-1), json!(10_000), json!(500.0), json!("500")] {
        assert_rejects::<RegisterListing>(
            with_field(&register, "expected_fee_bps", fee),
            "非法费率",
        );
    }
    let withdraw: Value = decode(public_fixture!("withdraw-request"));
    assert_eq!(
        serde_json::from_value::<WithdrawListing>(with_field(
            &withdraw,
            "expected_version",
            json!(9_007_199_254_740_991_i64)
        ))
        .unwrap()
        .expected_version
        .get(),
        9_007_199_254_740_991
    );
    for version in [json!(0), json!(-1), json!(9_007_199_254_740_992_i64), json!("1"), json!(1.5)] {
        assert_rejects::<WithdrawListing>(
            with_field(&withdraw, "expected_version", version),
            "非法预期版本",
        );
    }

    let receipt: Value = decode(public_fixture!("reprice-receipt"));
    assert_rejects::<CommandReceipt>(
        with_field(&receipt["data"], "price", json!(0)),
        "回执标价为零",
    );
    assert_rejects::<CommandReceipt>(
        with_field(&receipt["data"], "listing_version", json!(9_007_199_254_740_992_i64)),
        "回执版本超出安全整数",
    );
}

#[test]
fn pagination_keeps_empty_pages_and_offset_boundaries() {
    let empty = json!({"items": [], "total": 42, "page": 51, "page_size": 200});
    let decoded: ListingPage<ListingSummary> = serde_json::from_value(empty.clone()).unwrap();
    assert!(decoded.items.is_empty());
    assert_eq!((decoded.total, decoded.page, decoded.page_size), (42, 51, 200));
    assert_eq!(
        decode::<ApiResponse<ListingPage<ListingSummary>>>(public_fixture!("listings"))
            .data
            .page_size,
        37
    );
    for (field, value) in [
        ("page", json!(52)),
        ("page", json!(0)),
        ("page_size", json!(0)),
        ("page_size", json!(201)),
        ("total", json!(-1)),
        ("total", json!(9_007_199_254_740_992_i64)),
        ("total", json!("42")),
    ] {
        assert_rejects::<ListingPage<ListingSummary>>(with_field(&empty, field, value), "非法分页");
    }
    let edge =
        json!({"items": [], "total": 9_007_199_254_740_991_i64, "page": 10001, "page_size": 1});
    assert!(serde_json::from_value::<ListingPage<OwnedListing>>(edge).is_ok());
}

#[test]
fn identity_timestamp_and_equipment_boundaries_are_strict() {
    let detail: Value = decode(public_fixture!("listing-detail"));
    let summary = &detail["data"]["summary"];
    assert_eq!(
        decode::<ApiResponse<ListingDetail>>(public_fixture!("listing-detail"))
            .data
            .summary
            .external_equipment_id
            .len(),
        32
    );
    for id in [
        json!("0"),
        json!("042"),
        json!("４２"),
        json!("123456789012345678901234567890123"),
        json!(42),
    ] {
        assert_rejects::<ListingSummary>(
            with_field(summary, "external_equipment_id", id),
            "非规范装备编号",
        );
    }
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "00000000-0000-4000-8000-00000000000A",
        "00000000000040008000000000000004",
    ] {
        assert_rejects::<ListingSummary>(
            with_field(summary, "listing_id", json!(id)),
            "非规范 UUID",
        );
    }
    for instant in ["2026-11-20T10:10:00-08:00", "2026-11-20 18:10:00Z", "2026-02-30T18:10:00Z"] {
        assert_rejects::<ListingSummary>(
            with_field(summary, "updated_at", json!(instant)),
            "非 RFC 3339 UTC 时刻",
        );
    }
    for level in [json!(0), json!(100_001), json!("42"), json!(42.0)] {
        assert_rejects::<EquipmentLevel>(
            json!({"kind": "assigned", "value": level}),
            "等级值越界或类型错误",
        );
    }
    let stat = &detail["data"]["equipment"]["stat_groups"][0]["stats"][0];
    for number in [json!(1_000_000_001), json!("12.5"), Value::Null] {
        assert_rejects::<EquipmentStat>(with_field(stat, "value", number), "非法数值");
    }
}

#[test]
fn capabilities_and_admin_actions_fail_closed() {
    let status: Value = decode(public_fixture!("status"));
    let status = &status["data"];
    for (field, value) in [
        ("enabled", json!(false)),
        ("registration_ready", json!(false)),
        ("accepts_mail", json!(true)),
        ("purchasable", json!(true)),
        ("min_price", json!(0)),
    ] {
        assert_rejects::<ConsignmentStatus>(
            with_field(status, field, value),
            "不能宣告被禁用的能力",
        );
    }
    let mut status: ConsignmentStatus = serde_json::from_value(status.clone()).unwrap();
    status.purchasable = true;
    assert!(serde_json::to_value(status).is_err());
    let detail: Value = decode(public_fixture!("listing-detail"));
    assert_rejects::<ListingSummary>(
        with_field(&detail["data"]["summary"], "purchasable", json!(true)),
        "目录声称可购买",
    );
    let owned: Value = decode(public_fixture!("owned-listings"));
    assert_rejects::<ListingView>(
        with_field(&owned["data"]["items"][3]["view"], "accepts_mail", json!(true)),
        "待寄视图声称收件",
    );
    let admin: Value = decode(admin_fixture!("listings"));
    let mut active = admin["data"]["items"][1].clone();
    assert!(serde_json::from_value::<AdminListing>(active.clone()).unwrap().can_delist);
    active["listing"]["allowed_actions"] = json!(["reprice"]);
    assert_rejects::<AdminListing>(active, "Admin 获得卖家动作");
}

#[test]
fn stored_result_keeps_the_original_status_and_safe_code() {
    let accepted: ApiResponse<StoredCommandResult> = decode(public_fixture!("command-result"));
    assert_eq!(accepted.code, "ok");
    assert_eq!((accepted.data.http_status, accepted.data.code), (202, StoredCommandCode::Accepted));
    let rejected: ApiResponse<StoredCommandResult> = decode(public_fixture!("command-rejected"));
    assert_eq!(
        (rejected.data.http_status, rejected.data.code),
        (409, StoredCommandCode::FeeChanged)
    );
    assert_eq!(rejected.data.receipt.disposition, ListingDisposition::Rejected);
    let result = serde_json::to_value(rejected.data).unwrap();
    assert_rejects::<StoredCommandResult>(
        with_field(&result, "http_status", json!(200)),
        "外层状态覆盖原拒绝",
    );
    assert_rejects::<StoredCommandResult>(
        with_field(&result, "code", json!("command_id_conflict")),
        "命令 ID 冲突伪装成新账本结果",
    );
}

#[test]
fn admin_reason_is_bounded_and_sensitive_request_debug_is_redacted() {
    let register: RegisterListing = decode(public_fixture!("register-request"));
    let printed = format!("{register:?}");
    assert!(printed.contains("[REDACTED]"));
    assert!(!printed.contains(&register.equipment_url));
    let delist: AdminDelistListing = decode(admin_fixture!("delist-request"));
    assert!(!format!("{delist:?}").contains(&delist.reason));
    let wire: Value = decode(admin_fixture!("delist-request"));
    assert!(
        serde_json::from_value::<AdminDelistListing>(with_field(
            &wire,
            "reason",
            json!("a".repeat(256))
        ))
        .is_ok()
    );
    for reason in [String::new(), " trailing ".to_owned(), "one\ntwo".to_owned(), "界".repeat(86)]
    {
        assert_rejects::<AdminDelistListing>(
            with_field(&wire, "reason", json!(reason)),
            "非规范或超长下架说明",
        );
    }
}

#[test]
fn closed_sets_reject_non_string_json_tokens() {
    fn check<T: DeserializeOwned>(string: &str, object: &str) {
        let _: T = decode(string);
        for body in [object, r#"["consigned"]"#, "7", "null"] {
            assert!(serde_json::from_str::<T>(body).is_err(), "闭集必须解码自 JSON 字符串");
        }
    }

    check::<ListingMode>(r#""consigned""#, r#"{"consigned":null}"#);
    check::<ListingWorld>(r#""main""#, r#"{"main":null}"#);
    check::<PublicListingState>(r#""displayed""#, r#"{"displayed":null}"#);
    check::<ListingState>(r#""resolving""#, r#"{"resolving":null}"#);
    check::<ListingReason>(r#""lookup_deadline""#, r#"{"lookup_deadline":null}"#);
    check::<LookupReason>(r#""source_timeout""#, r#"{"source_timeout":null}"#);
    check::<ListingAction>(r#""reprice""#, r#"{"reprice":null}"#);
    check::<ListingOperation>(r#""register""#, r#"{"register":null}"#);
    check::<ListingDisposition>(r#""registered""#, r#"{"registered":null}"#);
    check::<StoredCommandCode>(r#""accepted""#, r#"{"accepted":null}"#);
    check::<EquipmentViewSource>(r#""hvdata-equip-v1""#, r#"{"hvdata-equip-v1":null}"#);

    for body in [r#"{"consigned":null}"#, r#"["consigned"]"#, "7", "null"] {
        let request = public_fixture!("register-request").replace(r#""consigned""#, body);
        assert!(serde_json::from_str::<RegisterListing>(&request).is_err());
    }
    for body in [r#"{"owner_mismatch":null}"#, r#"["owner_mismatch"]"#, "7", "null"] {
        let owned = public_fixture!("owned-listing-owner-mismatch")
            .replace(r#""reason": "owner_mismatch""#, &format!(r#""reason": {body}"#));
        assert!(serde_json::from_str::<ApiResponse<OwnedListing>>(&owned).is_err());
    }
    for body in [
        r#"{"kind":{"soulbound":null}}"#,
        r#"{"kind":["soulbound"]}"#,
        r#"{"kind":7}"#,
        r#"{"kind":null}"#,
    ] {
        assert!(serde_json::from_str::<EquipmentLevel>(body).is_err());
    }
    let owned: Value = decode(public_fixture!("owned-listing"));
    for kind in [json!({"resolving": null}), json!(["resolving"]), json!(7), Value::Null] {
        let view = with_field(&owned["data"]["view"], "kind", kind);
        assert!(serde_json::from_str::<ListingView>(&view.to_string()).is_err());
    }
}

#[test]
fn terminal_reason_codes_match_the_contract() {
    let cases = [
        (ListingReason::AuctionLotCreated, "auction_lot_created"),
        (ListingReason::SellerIneligible, "seller_ineligible"),
        (ListingReason::ServiceDisabled, "service_disabled"),
        (ListingReason::SellerCancelled, "seller_cancelled"),
        (ListingReason::AdminDelisted, "admin_delisted"),
        (ListingReason::LookupDeadline, "lookup_deadline"),
        (ListingReason::DisplayExpired, "display_expired"),
        (ListingReason::DepositExpired, "deposit_expired"),
        (ListingReason::NotTradeable, "not_tradeable"),
        (ListingReason::OwnerMissing, "owner_missing"),
        (ListingReason::OwnerMismatch, "owner_mismatch"),
        (ListingReason::HistoryOwnerMismatch, "history_owner_mismatch"),
        (ListingReason::HistoryAliasConflict, "history_alias_conflict"),
        (ListingReason::HistoryUnproven, "history_unproven"),
        (ListingReason::EquipmentBusy, "equipment_busy"),
        (ListingReason::EquipmentNotFound, "equipment_not_found"),
        (ListingReason::SourceRequestRejected, "source_request_rejected"),
        (ListingReason::SourceHttpInvalid, "source_http_invalid"),
        (ListingReason::SourceSchemaInvalid, "source_schema_invalid"),
        (ListingReason::SourceShapeUnconfirmed, "source_shape_unconfirmed"),
        (ListingReason::SourceIdentityMismatch, "source_identity_mismatch"),
        (ListingReason::SourceTooLarge, "source_too_large"),
        (ListingReason::LookupExhausted, "lookup_exhausted"),
        (ListingReason::CredentialUnavailable, "credential_unavailable"),
    ];
    for (reason, literal) in cases {
        let text = format!("\"{literal}\"");
        assert_eq!(decode::<ListingReason>(&text), reason);
        assert_eq!(serde_json::to_string(&reason).unwrap(), text);
    }
    assert_rejects::<ListingReason>(json!("owner_mismatched"), "拼错的原因码");
}

const SYNTHETIC_EQUIPMENT_URL: &str = "https://example.invalid/equip/42/a0b1c2d3e4";

#[test]
fn write_errors_report_only_safe_fields_and_categories() {
    fn check<T: DeserializeOwned>(payload: &Value, expected: &str) {
        let Some(error) = serde_json::from_str::<T>(&payload.to_string()).err() else {
            panic!("寄售写入应拒绝 {expected}");
        };
        assert!(!format!("{error}").contains(SYNTHETIC_EQUIPMENT_URL));
        assert!(!format!("{error:?}").contains(SYNTHETIC_EQUIPMENT_URL));
        assert!(format!("{error}").contains(expected), "写入错误缺少安全字段及类别");
    }

    let register: Value = decode(public_fixture!("register-request"));
    let url = json!(SYNTHETIC_EQUIPMENT_URL);
    for (field, diagnostic) in [
        ("price", "price 字段必须为范围内的 JSON 整数"),
        ("mode", "mode 字段必须为约定的模式字符串"),
        ("command_id", "command_id 字段必须为规范小写的非零 UUID"),
        ("expected_fee_bps", "expected_fee_bps 字段必须为范围内的 JSON 整数"),
    ] {
        check::<RegisterListing>(&with_field(&register, field, url.clone()), diagnostic);
    }
    check::<RegisterListing>(
        &with_field(&register, SYNTHETIC_EQUIPMENT_URL, Value::Null),
        "寄售写入含未知字段",
    );
    check::<RegisterListing>(&url, "寄售写入正文必须为 JSON 对象");
    let reprice: Value = decode(public_fixture!("reprice-request"));
    check::<RepriceListing>(
        &with_field(&reprice, "price", url.clone()),
        "price 字段必须为范围内的 JSON 整数",
    );
    let withdraw: Value = decode(public_fixture!("withdraw-request"));
    check::<WithdrawListing>(
        &with_field(&withdraw, "expected_version", url.clone()),
        "expected_version 字段必须为正的安全 JSON 整数",
    );

    let delist: Value = decode(admin_fixture!("delist-request"));
    check::<AdminDelistListing>(
        &with_field(&delist, "reason", url.clone()),
        "reason 字段必须为不含装备链接的规范单行文本",
    );
    check::<AdminDelistListing>(
        &with_field(&delist, "reason", json!([url])),
        "reason 字段必须为不含装备链接的规范单行文本",
    );
}

#[test]
fn operations_notes_reject_equipment_paths_and_hide_debug() {
    let request: Value = decode(admin_fixture!("delist-request"));
    let page: Value = decode(admin_fixture!("listings"));
    let record = &page["data"]["items"][0];
    let safe: AdminListing = serde_json::from_value(record.clone()).unwrap();
    let reason = safe.admin_delist_reason.as_deref().unwrap();
    let debug = format!("{safe:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(reason));

    for url in [
        SYNTHETIC_EQUIPMENT_URL,
        "https://example.invalid/ISEKAI/EqUiP/42/A0B1C2D3E4?origin=test",
        "/equip/17/CAFEBABE",
    ] {
        let note = format!("运营关闭（{url}）");
        let request_json = with_field(&request, "reason", json!(&note));
        assert_rejects::<AdminDelistListing>(request_json, "下架说明包含装备链接");
        let record_json = with_field(record, "admin_delist_reason", json!(&note));
        assert_rejects::<AdminListing>(record_json, "运营投影包含装备链接");

        let mut invalid = safe.clone();
        invalid.admin_delist_reason = Some(note.clone());
        assert!(!format!("{invalid:?}").contains(url));
        assert!(serde_json::to_string(&invalid).is_err());

        let mut request: AdminDelistListing = decode(admin_fixture!("delist-request"));
        request.reason = note;
        assert!(!format!("{request:?}").contains(url));
        assert!(serde_json::to_string(&request).is_err());
    }

    for note in ["登记重复，请人工复核", "请参阅 /equipment/42/a0b1c2d3e4"] {
        assert!(
            serde_json::from_value::<AdminDelistListing>(with_field(
                &request,
                "reason",
                json!(note)
            ))
            .is_ok()
        );
    }
}

#[test]
fn unavailable_projections_and_admin_actions_cannot_be_serialized() {
    let disabled: ApiResponse<ConsignmentStatus> = decode(public_fixture!("status-disabled"));
    let disabled = disabled.data;
    for (field, value) in [
        ("registration_ready", json!(true)),
        ("registration_modes", json!(["consigned"])),
        ("registration_worlds", json!(["main"])),
        ("accepts_mail", json!(true)),
        ("purchasable", json!(true)),
    ] {
        let object = serde_json::to_value(&disabled).unwrap();
        assert_rejects::<ConsignmentStatus>(with_field(&object, field, value), field);
    }
    let mut false_claim = disabled.clone();
    false_claim.registration_ready = true;
    assert!(serde_json::to_value(false_claim).is_err());
    let mut false_claim = disabled.clone();
    false_claim.registration_modes.push(ListingMode::Consigned);
    assert!(serde_json::to_value(false_claim).is_err());
    let mut false_claim = disabled.clone();
    false_claim.registration_worlds.push(ListingWorld::Main);
    assert!(serde_json::to_value(false_claim).is_err());
    let mut false_claim = disabled;
    false_claim.accepts_mail = true;
    assert!(serde_json::to_value(false_claim).is_err());

    let detail: ApiResponse<AdminListingDetail> = decode(admin_fixture!("listing-detail"));
    let mut admin = detail.data.record;
    let object = serde_json::to_value(&admin).unwrap();
    assert_rejects::<AdminListing>(with_field(&object, "can_delist", json!(true)), "终态可以下架");
    admin.can_delist = true;
    assert!(serde_json::to_value(admin).is_err());

    let page: ApiResponse<ListingPage<AdminListing>> = decode(admin_fixture!("listings"));
    let mut active = page.data.items[1].clone();
    active.listing.allowed_actions.push(ListingAction::Reprice);
    assert!(serde_json::to_value(active).is_err());
}

#[test]
fn owned_view_must_match_state_reason_actions_and_snapshot() {
    let resolving: ApiResponse<OwnedListing> = decode(public_fixture!("owned-listing"));
    let mut resolving = resolving.data;
    let object = serde_json::to_value(&resolving).unwrap();
    assert_rejects::<OwnedListing>(
        with_field(&object, "stored_state", json!("closed")),
        "持久终结后复活并提供卖家动作",
    );
    resolving.stored_state = ListingState::Closed;
    assert!(serde_json::to_value(&resolving).is_err());
    resolving.allowed_actions.clear();
    assert!(serde_json::to_value(resolving).is_err());

    let page: ApiResponse<ListingPage<OwnedListing>> = decode(public_fixture!("owned-listings"));
    let mut closed = page.data.items[4].clone();
    closed.allowed_actions.push(ListingAction::Withdraw);
    assert!(serde_json::to_value(&closed).is_err());
    closed.allowed_actions.clear();
    closed.stored_state = ListingState::Closed;
    let mut object = serde_json::to_value(&closed).unwrap();
    object["effective_reason"] = json!("lookup_deadline");
    object["view"]["reason"] = json!("lookup_deadline");
    assert_rejects::<OwnedListing>(object, "关闭使用到期原因");
    if let ListingView::Closed { reason, .. } = &mut closed.view {
        *reason = ListingReason::LookupDeadline;
    }
    closed.effective_reason = Some(ListingReason::LookupDeadline);
    assert!(serde_json::to_value(closed).is_err());

    let mismatch: ApiResponse<OwnedListing> =
        decode(public_fixture!("owned-listing-owner-mismatch"));
    let mut ineligible = mismatch.data;
    let object = serde_json::to_value(&ineligible).unwrap();
    let view = &object["view"];
    assert_rejects::<ListingView>(without_field(view, "equipment"), "已解析拒挂缺少快照");
    assert_rejects::<ListingView>(
        with_field(view, "reason", json!("credential_unavailable")),
        "无法读取凭据却携带已解析快照",
    );
    let mut object = object;
    object["effective_reason"] = json!("credential_unavailable");
    object["view"]["reason"] = json!("credential_unavailable");
    assert_rejects::<OwnedListing>(object, "已解析拒挂使用无快照的原因");
    let ListingView::Ineligible { reason, equipment } = &mut ineligible.view else {
        panic!("缺少有快照的拒挂夹具");
    };
    *reason = ListingReason::CredentialUnavailable;
    ineligible.effective_reason = Some(*reason);
    let equipment = serde_json::to_value(equipment).unwrap();
    assert!(serde_json::to_value(ineligible).is_err());

    let view = serde_json::to_value(&page.data.items[0].view).unwrap();
    assert_rejects::<ListingView>(with_field(&view, "equipment", equipment), "解析失败附带快照");
    assert_rejects::<ListingView>(
        with_field(&view, "reason", json!("not_tradeable")),
        "不可交易却无快照",
    );

    let displayed = serde_json::to_value(&page.data.items[2]).unwrap();
    assert_rejects::<OwnedListing>(
        with_field(&displayed, "stored_state", json!("resolving")),
        "有效展示早于持久发布",
    );
    assert_rejects::<OwnedListing>(
        with_field(&displayed, "stored_state", json!("awaiting_deposit")),
        "非寄售持久态为待寄",
    );
}

#[test]
fn public_catalog_rejects_mismatched_mode_and_untradeable_equipment() {
    let direct: ApiResponse<ListingDetail> = decode(public_fixture!("listing-detail"));
    let mut direct = direct.data;
    let summary = serde_json::to_value(&direct.summary).unwrap();
    assert_rejects::<ListingSummary>(
        with_field(&summary, "mode", json!("consigned")),
        "寄售挂单作为已展示",
    );
    let deposit: Value = decode(public_fixture!("listing-detail-awaiting-deposit"));
    assert_rejects::<ListingSummary>(
        with_field(&deposit["data"]["summary"], "mode", json!("direct")),
        "非寄售挂单作为待寄",
    );
    direct.summary.mode = ListingMode::Consigned;
    assert!(serde_json::to_value(&direct.summary).is_err());
    direct.summary.mode = ListingMode::Direct;

    let mut detail = serde_json::to_value(&direct).unwrap();
    detail["equipment"]["tradeable"] = json!(false);
    assert_rejects::<ListingDetail>(detail.clone(), "公开快照不可交易");
    detail["equipment"]["level"] = json!({"kind": "soulbound"});
    assert_rejects::<ListingDetail>(detail, "公开快照为魂绑");
    direct.equipment.tradeable = false;
    assert!(serde_json::to_value(&direct).is_err());

    let page: ApiResponse<ListingPage<OwnedListing>> = decode(public_fixture!("owned-listings"));
    let ListingView::Ineligible { equipment, .. } = &page.data.items[1].view else {
        panic!("缺少魂绑夹具");
    };
    let mut contradiction = equipment.clone();
    contradiction.tradeable = true;
    let object = serde_json::to_value(equipment).unwrap();
    assert_rejects::<EquipmentView>(
        with_field(&object, "tradeable", json!(true)),
        "魂绑声明可交易",
    );
    assert!(serde_json::to_value(contradiction).is_err());

    direct.summary.level = EquipmentLevel::Soulbound {};
    let object = with_field(&summary, "level", json!({"kind": "soulbound"}));
    assert_rejects::<ListingSummary>(object, "公开摘要为魂绑");
    assert!(serde_json::to_value(direct.summary).is_err());
}

#[test]
fn nullable_lookup_and_closed_fields_reject_missing_keys() {
    let owned: Value = decode(public_fixture!("owned-listing"));
    let progress = with_field(&owned["data"]["view"]["progress"], "next_attempt_at", Value::Null);
    let none = serde_json::from_value::<LookupProgress>(progress.clone()).unwrap();
    assert!(none.next_attempt_at.is_none());
    assert!(none.last_reason.is_none());
    for field in ["next_attempt_at", "last_reason"] {
        assert_rejects::<LookupProgress>(without_field(&progress, field), field);
    }

    let page: Value = decode(public_fixture!("owned-listings"));
    let closed = &page["data"]["items"][4]["view"];
    let nullable = with_field(closed, "equipment", Value::Null);
    assert!(matches!(
        serde_json::from_value::<ListingView>(nullable).unwrap(),
        ListingView::Closed { equipment: None, .. }
    ));
    assert_rejects::<ListingView>(without_field(closed, "equipment"), "关闭视图缺 equipment");
}

#[test]
fn progress_status_and_conflicts_enforce_protocol_limits() {
    let owned: ApiResponse<OwnedListing> = decode(public_fixture!("owned-listing"));
    let ListingView::Resolving { mut progress } = owned.data.view else {
        panic!("缺少解析中夹具");
    };
    progress.attempts = 6;
    let edge = serde_json::to_value(&progress).unwrap();
    assert!(serde_json::from_value::<LookupProgress>(edge.clone()).is_ok());
    for field in ["attempts", "max_attempts"] {
        assert_rejects::<LookupProgress>(with_field(&edge, field, json!(7)), field);
    }
    progress.attempts = 7;
    assert!(serde_json::to_value(&progress).is_err());
    progress.attempts = 6;
    progress.max_attempts = 7;
    assert!(serde_json::to_value(progress).is_err());

    let status: ApiResponse<ConsignmentStatus> = decode(public_fixture!("status"));
    let mut status = status.data;
    let object = serde_json::to_value(&status).unwrap();
    assert_eq!(object["max_price"], json!(1_000_000_000));
    assert_rejects::<ConsignmentStatus>(
        with_field(&object, "max_price", json!(1_000_000_001)),
        "登记价格上限超出协议",
    );
    status.max_price = 1_000_000_001;
    assert!(serde_json::to_value(status).is_err());

    let detail: ApiResponse<AdminListingDetail> = decode(admin_fixture!("listing-detail"));
    let mut detail = detail.data;
    let collision = detail.conflicts[0].clone();
    detail.conflicts.resize(50, collision.clone());
    let edge = serde_json::to_value(&detail).unwrap();
    assert!(serde_json::from_value::<AdminListingDetail>(edge.clone()).is_ok());
    let mut over = edge;
    over["conflicts"].as_array_mut().unwrap().push(serde_json::to_value(&collision).unwrap());
    assert_rejects::<AdminListingDetail>(over, "51 条碰撞");
    detail.conflicts.push(collision);
    assert!(serde_json::to_value(detail).is_err());
}

#[test]
fn owned_quote_checks_direct_zero_fee_floor_and_conservation() {
    let owned: ApiResponse<OwnedListing> = decode(public_fixture!("owned-listing"));
    let mut consigned = owned.data;
    assert_eq!((consigned.estimated_fee, consigned.estimated_seller_amount), (5, 96));
    let wire = serde_json::to_value(&consigned).unwrap();
    assert_rejects::<OwnedListing>(
        with_field(&wire, "mode", json!("direct")),
        "非寄售带 500 bps 与手续费",
    );
    let mut direct = consigned.clone();
    direct.mode = ListingMode::Direct;
    assert!(serde_json::to_value(&direct).is_err());
    direct.estimated_fee = 0;
    direct.estimated_seller_amount = 101;
    assert!(serde_json::to_value(&direct).is_err());
    direct.fee_bps = ListingFeeBps::new(0).unwrap();
    assert!(serde_json::to_value(&direct).is_ok());

    let mut wrong_rounding = wire.clone();
    wrong_rounding["estimated_fee"] = json!(6);
    wrong_rounding["estimated_seller_amount"] = json!(95);
    assert_rejects::<OwnedListing>(wrong_rounding, "手续费向上取整为 6");
    consigned.estimated_fee = 6;
    consigned.estimated_seller_amount = 95;
    assert!(serde_json::to_value(&consigned).is_err());
    consigned.estimated_fee = 5;
    assert!(serde_json::to_value(&consigned).is_err());
    assert_rejects::<OwnedListing>(
        with_field(&wire, "estimated_seller_amount", json!(95)),
        "到手金额与手续费不守恒",
    );

    for (field, value) in [
        ("price", json!(1_000_000_001)),
        ("version", json!(0)),
        ("pricing_version", json!(0)),
        ("pricing_version", json!(2)),
        ("fee_bps", json!(10_000)),
    ] {
        assert_rejects::<OwnedListing>(with_field(&wire, field, value), field);
    }
    assert!(ListingPrice::new(1_000_000_001).is_none());
    assert!(ListingVersion::new(0).is_none());
    assert!(ListingPricingVersion::new(0).is_none());
    assert!(ListingPricingVersion::new(2).is_none());
    assert!(ListingFeeBps::new(10_000).is_none());
}

#[test]
fn receipt_checks_operation_and_required_target_fields_for_each_disposition() {
    let registered: Value = decode(public_fixture!("register-receipt"));
    let repriced: Value = decode(public_fixture!("reprice-receipt"));
    let unchanged: Value = decode(public_fixture!("unchanged-receipt"));
    let withdrawn: Value = decode(public_fixture!("withdraw-receipt"));
    let delisted: Value = decode(admin_fixture!("delist-receipt"));
    for (body, wrong_op, wrong_state, needs_price) in [
        (registered, "withdraw", "displayed", true),
        (repriced, "register", "closed", true),
        (unchanged, "withdraw", "rejected", true),
        (withdrawn, "reprice", "resolving", false),
        (delisted, "register", "expired", false),
    ] {
        let body = &body["data"];
        assert_rejects::<CommandReceipt>(with_field(body, "operation", json!(wrong_op)), wrong_op);
        assert_rejects::<CommandReceipt>(
            with_field(body, "state", json!(wrong_state)),
            wrong_state,
        );
        for field in ["listing_id", "listing_version", "state"] {
            assert_rejects::<CommandReceipt>(with_field(body, field, Value::Null), field);
        }
        let no_price = with_field(body, "price", Value::Null);
        assert_eq!(serde_json::from_value::<CommandReceipt>(no_price).is_err(), needs_price);
    }
    let success: ApiResponse<CommandReceipt> = decode(public_fixture!("register-receipt"));
    let mut success = success.data;
    success.listing_version = ListingVersion::new(2);
    assert!(serde_json::to_value(&success).is_err());
    success.listing_version = ListingVersion::new(1);
    success.listing_id = None;
    assert!(serde_json::to_value(&success).is_err());

    let stored: Value = decode(public_fixture!("command-result"));
    let mut wrong_op = stored["data"].clone();
    wrong_op["receipt"]["operation"] = json!("withdraw");
    assert_rejects::<StoredCommandResult>(wrong_op, "202 登记回执却为卖家取消");
    let mut no_target = stored["data"].clone();
    no_target["receipt"]["listing_id"] = Value::Null;
    no_target["receipt"]["listing_version"] = Value::Null;
    assert_rejects::<StoredCommandResult>(no_target, "登记成功却缺目标与版本");
    let mut wrong_version = stored["data"].clone();
    wrong_version["receipt"]["listing_version"] = json!(2);
    assert_rejects::<StoredCommandResult>(wrong_version, "登记成功却不是版本 1");

    let rejected: Value = decode(public_fixture!("command-rejected"));
    let receipt = &rejected["data"]["receipt"];
    assert_rejects::<CommandReceipt>(receipt.clone(), "写成功回执出现 rejected");
    for operation in ["register", "reprice", "withdraw", "admin_delist"] {
        let mut rejected = rejected["data"].clone();
        rejected["receipt"]["operation"] = json!(operation);
        assert!(serde_json::from_value::<StoredCommandResult>(rejected).is_ok());
    }
}

#[test]
fn stored_result_rejects_success_codes_with_a_different_disposition() {
    let accepted: Value = decode(public_fixture!("command-result"));
    let accepted = &accepted["data"];
    let reprice: Value = decode(public_fixture!("reprice-receipt"));
    let repriced = with_field(accepted, "receipt", reprice["data"].clone());
    assert_rejects::<StoredCommandResult>(repriced.clone(), "202 accepted 配 repriced");
    let ok = with_field(&with_field(&repriced, "http_status", json!(200)), "code", json!("ok"));
    assert!(serde_json::from_value::<StoredCommandResult>(ok).is_ok());
    let ok_but_registered =
        with_field(&with_field(accepted, "http_status", json!(200)), "code", json!("ok"));
    assert_rejects::<StoredCommandResult>(ok_but_registered, "200 ok 配 registered");
    assert_rejects::<StoredCommandResult>(
        with_field(accepted, "http_status", json!(200)),
        "accepted 与 200 不符",
    );

    let mut accepted: StoredCommandResult = serde_json::from_value(accepted.clone()).unwrap();
    accepted.receipt.operation = ListingOperation::Withdraw;
    assert!(serde_json::to_value(&accepted).is_err());
    accepted.receipt.operation = ListingOperation::Register;
    accepted.http_status = 200;
    accepted.code = StoredCommandCode::Ok;
    assert!(serde_json::to_value(accepted).is_err());

    let rejected: Value = decode(public_fixture!("command-rejected"));
    assert_rejects::<StoredCommandResult>(
        with_field(&rejected["data"], "http_status", json!(422)),
        "拒绝状态与安全结果码不符",
    );
}
