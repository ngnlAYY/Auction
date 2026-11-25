//! 受限权利 DTO 的共享夹具往返、严格性与秘密脱敏回归；TypeScript 解码同一批夹具。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::ApiResponse;
use auction_contracts::ChallengeRightRequest;
use auction_contracts::ClaimRightRequest;
use auction_contracts::LocateRightRequest;
use auction_contracts::RightChallengeAccepted;
use auction_contracts::RightClaimReceipt;
use auction_contracts::RightVerifiedGrant;
use auction_contracts::RightViewV2;
use auction_contracts::VerifyRightRequest;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::json;

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/public/consignment-rights-", $name, ".json"))
    };
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str, fixture: &str) {
    let expected: Value = serde_json::from_str(fixture).expect("夹具应为 JSON");
    let decoded: T = serde_json::from_str(fixture)
        .unwrap_or_else(|error| panic!("夹具 {name} 解码失败（{:?}）", error.classify()));
    assert!(serde_json::to_value(decoded).expect("序列化") == expected, "夹具 {name} 往返变化");
}

#[test]
fn rights_fixtures_round_trip() {
    round_trip::<LocateRightRequest>("locate-request", fixture!("locate-request"));
    round_trip::<ChallengeRightRequest>("challenge-request", fixture!("challenge-request"));
    round_trip::<VerifyRightRequest>("verify-request", fixture!("verify-request"));
    round_trip::<ClaimRightRequest>("claim-request", fixture!("claim-request"));
    round_trip::<ApiResponse<RightChallengeAccepted>>(
        "challenge-accepted",
        fixture!("challenge-accepted"),
    );
    round_trip::<ApiResponse<RightVerifiedGrant>>("verified", fixture!("verified"));
    round_trip::<ApiResponse<RightViewV2>>("view", fixture!("view"));
    round_trip::<ApiResponse<RightClaimReceipt>>("claim-receipt", fixture!("claim-receipt"));
}

#[test]
fn rights_requests_are_closed_and_reject_non_v4_command_ids() {
    let mut request: Value = serde_json::from_str(fixture!("claim-request")).unwrap();
    request["extra"] = json!(1);
    assert!(serde_json::from_value::<ClaimRightRequest>(request).is_err());
    let mut request: Value = serde_json::from_str(fixture!("claim-request")).unwrap();
    request["command_id"] = json!("00000000-0000-5000-8000-000000000504");
    assert!(serde_json::from_value::<ClaimRightRequest>(request).is_err());
    let mut request: Value = serde_json::from_str(fixture!("claim-request")).unwrap();
    request["action"] = json!("admin_override");
    assert!(serde_json::from_value::<ClaimRightRequest>(request).is_err());
}

#[test]
fn rights_view_rejects_actions_foreign_to_its_kind() {
    let mut view: Value = serde_json::from_str::<Value>(fixture!("view")).unwrap()["data"].clone();
    view["allowed_actions"] = json!(["claim_equipment"]);
    assert!(serde_json::from_value::<RightViewV2>(view.clone()).is_err());
    view["kind"] = json!("loss_case");
    view["allowed_actions"] = json!(["claim_payment"]);
    assert!(serde_json::from_value::<RightViewV2>(view).is_err());
}

#[test]
fn rights_secrets_are_redacted_from_debug_and_decode_errors() {
    let locator = format!("R1-{}", "Q".repeat(43));
    let request: ChallengeRightRequest = serde_json::from_value(json!({
        "command_id": "00000000-0000-4000-8000-000000000502",
        "right_locator": locator,
    }))
    .unwrap();
    assert!(!format!("{request:?}").contains(&locator));
    let verify: VerifyRightRequest = serde_json::from_str(fixture!("verify-request")).unwrap();
    assert!(!format!("{verify:?}").contains(&verify.code));
    let error = serde_json::from_value::<ChallengeRightRequest>(json!({
        "command_id": "00000000-0000-4000-8000-000000000502",
        "right_locator": "bad-locator-value-xyz",
    }))
    .unwrap_err();
    assert!(!error.to_string().contains("bad-locator-value-xyz"));
}
