//! 第 5 期 v2 本人 / Admin 读模型的共用夹具往返与一致性拒绝。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::AdminListingDetailV2;
use auction_contracts::AdminListingPageV2;
use auction_contracts::AdminListingV2;
use auction_contracts::ApiResponse;
use auction_contracts::OwnedListingPageV2;
use auction_contracts::OwnedListingV2;
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
    serde_json::from_str(fixture).expect("夹具应可解码")
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str, fixture: &str) {
    let expected: Value = decode(fixture);
    let decoded: T = decode(fixture);
    let actual = serde_json::to_value(decoded).expect("DTO 应可序列化");
    assert!(actual == expected, "夹具 {name} 的往返结果发生变化");
}

fn rejects<T: DeserializeOwned>(wire: Value, scenario: &str) {
    assert!(serde_json::from_value::<T>(wire).is_err(), "应拒绝 {scenario}");
}

fn admin_record() -> Value {
    decode::<Value>(admin_fixture!("listing-v2-detail"))["data"]["record"].clone()
}

#[test]
fn v2_read_fixtures_round_trip() {
    round_trip::<ApiResponse<OwnedListingPageV2>>(
        "listings-v2",
        public_fixture!("owned-listings-v2"),
    );
    round_trip::<ApiResponse<OwnedListingV2>>(
        "listing-v2-held-accepted",
        public_fixture!("listing-v2-held-accepted"),
    );
    round_trip::<ApiResponse<AdminListingPageV2>>(
        "admin listings-v2",
        admin_fixture!("listings-v2"),
    );
    round_trip::<ApiResponse<AdminListingDetailV2>>(
        "admin listing-v2-detail",
        admin_fixture!("listing-v2-detail"),
    );
}

/// 对已接受、尚未取得的件设运营止售会进入 held_off_market，此时没有处分代；
/// 已上架态则必须有处分代。
#[test]
fn held_off_market_may_lack_a_generation_but_available_may_not() {
    let held: Value = decode::<Value>(public_fixture!("listing-v2-held-accepted"))["data"].clone();
    assert_eq!(held["generation"], Value::Null);
    serde_json::from_value::<OwnedListingV2>(held).expect("止售的已接受件没有处分代");
    let mut available: Value =
        decode::<Value>(public_fixture!("owned-listings-v2"))["data"]["items"][0].clone();
    assert_eq!(available["view"]["kind"], "available");
    available["generation"] = Value::Null;
    rejects::<OwnedListingV2>(available, "没有处分代的 available");

    // 本人视图不带 position 状态，只有运营止售可无代；其余原因的 held 必须有代。
    let mut quota: Value =
        decode::<Value>(public_fixture!("listing-v2-held-accepted"))["data"].clone();
    quota["effective_reason"] = json!("quota_blocked");
    quota["view"]["reason"] = json!("quota_blocked");
    quota["allowed_actions"] = json!(["withdraw", "relist"]);
    rejects::<OwnedListingV2>(quota, "没有处分代的非运营止售 held");
}

#[test]
fn admin_record_rejects_inconsistent_hold_and_actions() {
    let record = admin_record();
    serde_json::from_value::<AdminListingV2>(record.clone()).expect("夹具记录合法");

    let mut with_seller_actions = record.clone();
    with_seller_actions["listing"]["allowed_actions"] = json!(["withdraw"]);
    rejects::<AdminListingV2>(with_seller_actions, "Admin 记录携带卖家动作");

    let mut hold_missing = record.clone();
    hold_missing["hold"] = Value::Null;
    rejects::<AdminListingV2>(hold_missing, "允许解除止售而记录无止售");

    // hold_id 只在 Admin 的 hold 展示，在库下架以外的状态不得带有效止售。
    let free: Value = decode::<Value>(admin_fixture!("listings-v2"))["data"]["items"][1].clone();
    let mut hold_outside_held = free;
    hold_outside_held["hold"] = record["hold"].clone();
    hold_outside_held["admin_actions"] = json!([]);
    rejects::<AdminListingV2>(hold_outside_held, "非在库下架态却带有效止售");

    // 只有 position 处于 awaiting_take 的在库下架件可以没有处分代。
    let mut taken_without_generation = record.clone();
    taken_without_generation["listing"]["generation"] = Value::Null;
    taken_without_generation["position"]["generation"] = Value::Null;
    rejects::<AdminListingV2>(taken_without_generation, "已取得的 held 无处分代");
    let mut awaiting_take = record.clone();
    awaiting_take["listing"]["generation"] = Value::Null;
    awaiting_take["position"]["generation"] = Value::Null;
    awaiting_take["position"]["state"] = json!("awaiting_take");
    serde_json::from_value::<AdminListingV2>(awaiting_take).expect("等待取得的 held 可无处分代");

    let mut both = record.clone();
    both["admin_actions"] = json!(["delist", "release_hold"]);
    rejects::<AdminListingV2>(both, "下架与解除止售同时允许");

    let mut delist_under_hold = record.clone();
    delist_under_hold["admin_actions"] = json!(["delist"]);
    rejects::<AdminListingV2>(delist_under_hold, "有效止售下仍允许下架");

    let mut duplicate = record.clone();
    duplicate["admin_actions"] = json!(["release_hold", "release_hold"]);
    rejects::<AdminListingV2>(duplicate, "重复的 Admin 动作");

    let mut generation_mismatch = record;
    generation_mismatch["position"]["generation"] = json!(2);
    rejects::<AdminListingV2>(generation_mismatch, "position 处分代与挂单不一致");
}

#[test]
fn admin_release_requires_a_hold_in_the_held_state() {
    let free: Value = decode::<Value>(admin_fixture!("listings-v2"))["data"]["items"][1].clone();
    serde_json::from_value::<AdminListingV2>(free.clone()).expect("无止售的可下架记录");
    let mut release = free;
    release["admin_actions"] = json!(["release_hold"]);
    rejects::<AdminListingV2>(release, "没有止售却允许解除");
}
