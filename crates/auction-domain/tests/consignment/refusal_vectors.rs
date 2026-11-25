//! 拒收事实、周期与处罚 ID 的冻结向量。
//!
//! 向量由独立实现（Python 的 SHA-256 长度前缀算法）核算；namespace、字节序或参数顺序变化
//! 必须让本文件失败。v5 字节一旦冻结不得再改。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::refusal::{cycle_id, return_fact_id, sanction_id};
use auction_domain::{SettlementWorld, UserId};
use uuid::Uuid;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("测试 UUID")
}

fn buyer() -> UserId {
    UserId::from_uuid(uuid("66666666-6666-4666-8666-666666666666"))
}

#[test]
fn return_fact_ids_match_frozen_vectors() {
    let main = OriginalMailKey::new("42", SettlementWorld::Main, "100").unwrap();
    let isekai = OriginalMailKey::new("42", SettlementWorld::Isekai, "9007199254740993").unwrap();
    assert_eq!(return_fact_id(&main, 1, "original"), uuid("272e224d-02fb-5442-b73d-e4bef4712189"));
    assert_eq!(return_fact_id(&isekai, 2, "carrier"), uuid("be54c386-55c9-5a12-895e-8698552c0b80"));
    // 反例：轮次、腿或 K 任一不同，事实 ID 必变（重复 RTS 才会落到同一事实）。
    let base = return_fact_id(&main, 1, "original");
    assert_ne!(base, return_fact_id(&main, 2, "original"));
    assert_ne!(base, return_fact_id(&main, 1, "carrier"));
    assert_ne!(base, return_fact_id(&isekai, 1, "original"));
}

#[test]
fn cycle_and_sanction_ids_match_frozen_vectors() {
    let first = cycle_id(buyer(), 1);
    assert_eq!(first, uuid("e166e8ad-1a9e-5b62-ad1e-df2d10f71e30"));
    assert_eq!(cycle_id(buyer(), 2), uuid("95db3a13-178c-5edf-aca3-c74734bf1c0f"));
    assert_eq!(sanction_id(buyer(), first), uuid("d3d5be0e-b6c6-5d41-938a-7d3f8380a324"));
    // 反例：解禁后的新周期得到不同的处罚键，旧处罚键不会复用。
    assert_ne!(sanction_id(buyer(), cycle_id(buyer(), 2)), sanction_id(buyer(), first));
}
