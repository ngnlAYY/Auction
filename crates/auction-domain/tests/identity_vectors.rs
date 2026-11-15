//! 长度前缀旧版稳定身份的冻结向量。
//!
//! 结算义务、履约投递、恢复主键、周排期、结算工作流与慢速代理命令身份都按这两种算法写入
//! 数据库；任何实现改动都必须让下列向量逐字不变。向量由各调用方合并到领域共享实现之前的
//! 代码生成。

use auction_domain::{legacy_natural_key_uuid, legacy_worker_uuid};
use uuid::{Uuid, Variant};

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|error| panic!("测试 UUID 必须合法: {error}"))
}

fn assert_v5(derived: Uuid) {
    assert_eq!(derived.get_version_num(), 5);
    assert_eq!(derived.get_variant(), Variant::RFC4122);
}

#[test]
fn natural_key_uuid_matches_frozen_vectors() {
    let obligation = uuid("3d7e3f2a-4c1e-4a4c-9a02-1c9a5f0d3b21");
    let cases: [(&str, Vec<&[u8]>, &str); 6] = [
        (
            "auction-schedule",
            vec![b"round", b"world-main", b"2026-11-09"],
            "5239fe8f-6d40-50af-b5e9-1864ce96bab6",
        ),
        (
            "auction-schedule",
            vec![b"auction", b"world-main", b"early-bird:2026-11-09:1"],
            "3e0be2af-ce07-5401-909b-59d28915af2b",
        ),
        (
            "auction.fulfillment.delivery.v1",
            vec![obligation.as_bytes()],
            "49004e8c-030a-54e4-a260-ff0eeb791390",
        ),
        (
            "auction.recovery.relist.v1",
            vec![obligation.as_bytes(), &[0, 0, 0, 0, 0, 0, 0, 7]],
            "bdb75395-e5dd-57bf-8018-8ef861d9e59b",
        ),
        ("", Vec::new(), "af5570f5-a181-5b7a-b78c-af4bc70a660f"),
        // 空片段也写入八字节长度，与“没有片段”是不同身份。
        ("namespace", vec![b""], "77fb6eea-7e39-5b55-a18b-98a585354996"),
    ];
    for (namespace, parts, expected) in cases {
        let derived = legacy_natural_key_uuid(namespace, &parts);
        assert_eq!(derived, uuid(expected), "{namespace}");
        assert_v5(derived);
    }
}

#[test]
fn worker_uuid_matches_frozen_vectors() {
    let auction_id = uuid("0f8b2c4e-7a1d-4e3b-9c5f-2d6a8e0b1c3d");
    let authorization_id = uuid("2e4d6f8a-1b3c-4e5d-9f7a-8c6b4a2e0d1f");
    // 1_794_659_696_123_456_789 = 2026-11-14T12:34:56.123456789Z；负值覆盖纪元前的补码编码。
    for (namespace, kind, entity_id, due_nanos, expected) in [
        (
            "auction-settlement",
            "workflow",
            auction_id,
            1_794_659_696_123_456_789,
            "a2cfde74-e3a0-5ddc-a6f7-c53ac0359d15",
        ),
        (
            "auction-worker",
            "settlement_claim",
            auction_id,
            1_794_659_696_123_456_789,
            "2b52d9e6-4af0-560e-b9c7-4c8e64b9b4fe",
        ),
        (
            "auction-worker",
            "settlement_complete",
            auction_id,
            1_794_659_696_123_456_789,
            "76a6a45f-06e2-5460-84cb-f200bbdc6910",
        ),
        (
            "auction-settlement",
            "workflow",
            auction_id,
            -500_000_000,
            "33da5381-920e-53e9-b406-ab63086e1635",
        ),
        (
            "auction-worker",
            "slow_proxy:3",
            authorization_id,
            1_794_659_696_123_456_789,
            "70195ee3-a866-5df8-a6ee-7cbe8e42e3a1",
        ),
        (
            "auction-worker",
            "slow_proxy:1",
            authorization_id,
            1_794_657_600_000_000_000,
            "d4e024ae-f45e-5abf-bb5a-623966d88d57",
        ),
    ] {
        let derived = legacy_worker_uuid(namespace, kind, entity_id, due_nanos);
        assert_eq!(derived, uuid(expected), "{namespace}/{kind}/{due_nanos}");
        assert_v5(derived);
    }
}
