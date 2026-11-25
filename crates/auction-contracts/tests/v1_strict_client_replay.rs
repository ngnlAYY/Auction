//! 旧严格客户端回放：v1 响应形状在 R4 5Va-0 可表达性过滤后仍可原样解码。
//!
//! 服务端过滤（列表不含不可表达行、`total` 同过滤同快照、详情 404）由
//! `auction-storage` 的 `postgres_v1_owned_list_hides_inexpressible_rows` 覆盖，这里只证明
//! 过滤后的响应是旧解码器接受的，且未过滤的代管态或 `season_closed` 行会被旧解码器拒绝。
//! 旧页面的严格解码器就是这些 v1 DTO 的 `Deserialize`（`auction-web-public` 尚无独立的寄售解码路径）。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_contracts::ApiResponse;
use auction_contracts::ListingPage;
use auction_contracts::ListingReason;
use auction_contracts::ListingState;
use auction_contracts::OwnedListing;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const LIST: &str = include_str!("fixtures/public/consignment-v1-owned-listings-filtered.json");
const DETAIL: &str = include_str!("fixtures/public/consignment-v1-owned-detail-closed.json");
const ESCROW_ROW: &str = include_str!("fixtures/public/consignment-v1-owned-row-escrow.json");
const SEASON_CLOSED_ROW: &str =
    include_str!("fixtures/public/consignment-v1-owned-row-season-closed.json");

/// 解码失败只报告分类，不回显夹具正文。
fn decode<T: DeserializeOwned>(fixture: &str) -> T {
    serde_json::from_str(fixture).unwrap_or_else(|error| {
        panic!("v1 夹具解码失败（{:?}，第 {} 行）", error.classify(), error.line())
    })
}

/// 解码后重新序列化必须与夹具逐字节一致，夹具才算 v1 DTO 的真实线格式。
fn assert_byte_identical<T: DeserializeOwned + Serialize>(fixture: &str) {
    let decoded: T = decode(fixture);
    let mut actual = serde_json::to_string_pretty(&decoded).expect("v1 DTO 应可序列化");
    actual.push('\n');
    assert!(actual == fixture, "v1 夹具与 DTO 序列化不一致");
}

/// 把一行塞进过滤后的列表，`total` 同步加一，使失败只能来自该行。
fn list_with_row(row: &str) -> Value {
    let mut list: Value = decode(LIST);
    let row: Value = decode(row);
    list["data"]["items"].as_array_mut().expect("items 是数组").push(row);
    let total = list["data"]["total"].as_i64().expect("total 是整数");
    list["data"]["total"] = Value::from(total + 1);
    list
}

#[test]
fn v1_filtered_owned_list_and_detail_decode_byte_identically() {
    assert_byte_identical::<ApiResponse<ListingPage<OwnedListing>>>(LIST);
    assert_byte_identical::<ApiResponse<OwnedListing>>(DETAIL);
    let list: ApiResponse<ListingPage<OwnedListing>> = decode(LIST);
    let states: Vec<_> =
        list.data.items.iter().map(|item| (item.stored_state, item.effective_reason)).collect();
    assert_eq!(
        states,
        [
            (ListingState::AwaitingDeposit, None),
            (ListingState::Closed, Some(ListingReason::AdminDelisted)),
        ]
    );
}

#[test]
fn v1_strict_decoder_rejects_unfiltered_escrow_and_season_closed_rows() {
    // 先证明夹具加行后只差这一行：基线可解码，加入不可表达行后必须失败。
    assert!(serde_json::from_value::<ApiResponse<ListingPage<OwnedListing>>>(decode(LIST)).is_ok());
    for (scenario, row) in [("代管态", ESCROW_ROW), ("closed/season_closed", SEASON_CLOSED_ROW)]
    {
        let polluted = list_with_row(row);
        assert!(
            serde_json::from_value::<ApiResponse<ListingPage<OwnedListing>>>(polluted).is_err(),
            "旧严格解码器应拒绝未过滤的{scenario}行"
        );
    }
}
