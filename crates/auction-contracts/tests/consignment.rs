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
use auction_contracts::ListingAction;
use auction_contracts::ListingDetail;
use auction_contracts::ListingDisposition;
use auction_contracts::ListingMode;
use auction_contracts::ListingOperation;
use auction_contracts::ListingPage;
use auction_contracts::ListingReason;
use auction_contracts::ListingState;
use auction_contracts::ListingSummary;
use auction_contracts::ListingView;
use auction_contracts::LookupReason;
use auction_contracts::OwnedListing;
use auction_contracts::RegisterListing;
use auction_contracts::RepriceListing;
use auction_contracts::StoredCommandCode;
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
    assert_eq!(items[4].version, 2);
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
        assert_rejects::<CommandReceipt>(without_field(&receipt, field), "拒绝回执缺少可空键");
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
                .price,
            price
        );
        assert_eq!(
            serde_json::from_value::<RepriceListing>(with_field(&reprice, "price", json!(price)))
                .unwrap()
                .price,
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
            .expected_fee_bps,
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
        .expected_version,
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
