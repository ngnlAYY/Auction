//! 已持久化稳定身份与幂等摘要的冻结向量。
//!
//! 结算工作流/领取/完成命令 ID 与慢速代理命令 ID 会写入数据库并被重试、重放与 Worker
//! bootstrap 重新派生；算法或输入编码一旦漂移，同一事实会得到两个身份。向量由合并到领域
//! 共享算法之前的实现生成，之后任何改动都必须保持这些值逐字不变。竞价与 recovery 确认命令的
//! 请求摘要写入命令账本，同一命令 ID 重放时据此区分“同请求”与“冲突”；其向量由与 v0.1.4
//! 发布版逐字相同的实现生成。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_application::auction::{
    CreateProxyCommand, LowerProxyCommand, PlaceBidCommand, SlowProxyDue,
};
use auction_application::core::RequestHash;
use auction_application::recovery::{RecoveryConfirmationAction, RecoveryConfirmationCommand};
use auction_application::settlement::{
    settlement_claim_command_id, settlement_complete_command_id, settlement_complete_request_hash,
    settlement_workflow_id,
};
use auction_domain::{BidAmount, CommandId, LotId, ProxyAuthorizationId, ProxyMode, UserId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|error| panic!("测试 UUID 必须合法: {error}"))
}

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("测试时间必须合法: {error}"))
        .with_timezone(&Utc)
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

const AUCTION_ID: &str = "0f8b2c4e-7a1d-4e3b-9c5f-2d6a8e0b1c3d";
const WORKFLOW_ID: &str = "5b1e9c7a-3d2f-4a6b-8e0c-1f4d7a9b2c6e";

#[test]
fn settlement_worker_identities_match_frozen_vectors() {
    let auction_id = uuid(AUCTION_ID);
    // 第二组 due 早于 Unix 纪元：纳秒为负数时仍按 8 字节大端补码写入摘要。
    for (due_at, workflow, claim, complete, complete_hash) in [
        (
            "2026-11-14T12:34:56.123456789Z",
            "a2cfde74-e3a0-5ddc-a6f7-c53ac0359d15",
            "2b52d9e6-4af0-560e-b9c7-4c8e64b9b4fe",
            "76a6a45f-06e2-5460-84cb-f200bbdc6910",
            "381b54a5fe12c467ef27a92668feb0d09cd706a550f98eba623580d982f59c5b",
        ),
        (
            "1969-12-31T23:59:59.5Z",
            "33da5381-920e-53e9-b406-ab63086e1635",
            "51c6e867-d608-500e-a9e8-62b67196028c",
            "488eb074-d018-5324-848a-a0a47fe4a448",
            "14074370c18f6cb165691efdfdc4c3b13e975e6e08da0b3ad19646d4d07a2aa4",
        ),
    ] {
        let due = at(due_at);
        assert_eq!(settlement_workflow_id(auction_id, due), Some(uuid(workflow)), "{due_at}");
        assert_eq!(
            settlement_claim_command_id(auction_id, due).map(CommandId::as_uuid),
            Some(uuid(claim)),
            "{due_at}"
        );
        assert_eq!(
            settlement_complete_command_id(auction_id, due).map(CommandId::as_uuid),
            Some(uuid(complete)),
            "{due_at}"
        );
        let hash = settlement_complete_request_hash(auction_id, uuid(WORKFLOW_ID), due)
            .unwrap_or_else(|error| panic!("完成命令摘要必须可编码: {error}"));
        assert_eq!(hex(hash.as_bytes()), complete_hash, "{due_at}");
    }
}

#[test]
fn slow_proxy_command_identity_matches_frozen_vectors() {
    for (generation, due_at, command_id, request_hash) in [
        (
            3_u64,
            "2026-11-14T12:34:56.123456789Z",
            "70195ee3-a866-5df8-a6ee-7cbe8e42e3a1",
            "aca4cf8416ee70e1a34343d5230f489a3e50f8616d883e2970e2ef8c5d7b8125",
        ),
        (
            1,
            "2026-11-14T12:00:00Z",
            "d4e024ae-f45e-5abf-bb5a-623966d88d57",
            "dfa5350d1ecdf58496c6821832ad348b0f929806e54a81aeb3d9795490f00f28",
        ),
    ] {
        let command = SlowProxyDue {
            lot_id: LotId::from_uuid(uuid("7c3a1e5d-9b2f-4d8a-a6c4-3e1f5b7d9a2c")),
            authorization_id: ProxyAuthorizationId::from_uuid(uuid(
                "2e4d6f8a-1b3c-4e5d-9f7a-8c6b4a2e0d1f",
            )),
            generation,
            due_at: at(due_at),
        }
        .into_command()
        .unwrap_or_else(|error| panic!("慢速代理候选必须合法: {error}"));
        assert_eq!(command.command_id.as_uuid(), uuid(command_id), "{generation}");
        assert_eq!(hex(command.request_hash.as_bytes()), request_hash, "{generation}");
    }
}

const LOT_ID: &str = "7c3a1e5d-9b2f-4d8a-a6c4-3e1f5b7d9a2c";
const BIDDER_ID: &str = "9a4c2e6b-5d1f-4b3a-8c7e-0f2d4b6a8c1e";
const AUTHORIZATION_ID: &str = "2e4d6f8a-1b3c-4e5d-9f7a-8c6b4a2e0d1f";
const RECOVERY_ID: &str = "c1e3a5b7-9d2f-4c6e-8a0b-3d5f7b9e1a2c";

fn amount(value: i64) -> BidAmount {
    BidAmount::new(value).unwrap_or_else(|error| panic!("测试金额必须合法: {error}"))
}

fn encoded(hash: Result<RequestHash, serde_json::Error>) -> String {
    hex(hash.unwrap_or_else(|error| panic!("请求摘要必须可编码: {error}")).as_bytes())
}

/// 普通出价、两种模式的代理创建与代理降额各一组。
#[test]
fn competition_request_hashes_match_frozen_vectors() {
    let lot_id = LotId::from_uuid(uuid(LOT_ID));
    let bidder_id = UserId::from_uuid(uuid(BIDDER_ID));
    let authorization_id = ProxyAuthorizationId::from_uuid(uuid(AUTHORIZATION_ID));
    let actual = [
        encoded(PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount(1_234_000))),
        encoded(CreateProxyCommand::legacy_request_hash(
            lot_id,
            bidder_id,
            ProxyMode::Fast,
            amount(5_000_000),
        )),
        encoded(CreateProxyCommand::legacy_request_hash(
            lot_id,
            bidder_id,
            ProxyMode::Slow,
            amount(5_000_000),
        )),
        encoded(LowerProxyCommand::legacy_request_hash(
            lot_id,
            bidder_id,
            authorization_id,
            amount(4_000_000),
        )),
    ];
    assert_eq!(
        actual,
        [
            "b91db674713312a8dae955ad1d573f745460d31cec57a0920f0f4e99316840d9",
            "f7fe453fbbd0ab04c60205b18183d3f912db7c4e65848e5efd5e08765c5f170a",
            "f58581c7b970a6b6dc31f0bf892c18d5a33bbd31ac37948ca53eef9861a3f304",
            "c3d367001f327e3221d425805e9353f0af68d87c79104095ad7bf1f03a2b82fa",
        ]
    );
}

/// 摘要只取 `{RecoveryID,Version,Action,Reason}`，Reason 去首尾空白后参与；非 ASCII 与 HTML
/// 敏感字符的编码一并固定。
#[test]
fn recovery_confirmation_request_hash_matches_frozen_vectors() {
    let recovery_id = uuid(RECOVERY_ID);
    let hash = |action, reason: &str| {
        encoded(RecoveryConfirmationCommand::legacy_request_hash(recovery_id, 7, action, reason))
    };
    let actual = [
        hash(RecoveryConfirmationAction::Reclaimed, "玩家已取回装备"),
        hash(RecoveryConfirmationAction::Cancelled, "operator cancelled"),
        hash(RecoveryConfirmationAction::EquipmentRecovered, "equipment back <&>"),
    ];
    assert_eq!(
        actual,
        [
            "ef898bd4fc11f9392650429815cbdd95f0e54837b2fca0bc77471a5b4581cddc",
            "5aafcb6a08a9a5a925192d81c6c6b512381dcd7148a5855122334add5abbaf74",
            "d65e0cd51d73f0074d3d1bbba38ce6a8dc0e10f2911b2702a412283e3c18a5f2",
        ]
    );
    assert_eq!(hash(RecoveryConfirmationAction::Reclaimed, "  玩家已取回装备\n"), actual[0]);
}
