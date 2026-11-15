//! 已持久化稳定身份与幂等摘要的冻结向量。
//!
//! 结算工作流/领取/完成命令 ID 与慢速代理命令 ID 会写入数据库并被重试、重放与 Worker
//! bootstrap 重新派生；算法或输入编码一旦漂移，同一事实会得到两个身份。向量由合并到领域
//! 共享算法之前的实现生成，之后任何改动都必须保持这些值逐字不变。

use auction_application::auction::SlowProxyDue;
use auction_application::settlement::{
    settlement_claim_command_id, settlement_complete_command_id, settlement_complete_request_hash,
    settlement_workflow_id,
};
use auction_domain::{CommandId, LotId, ProxyAuthorizationId};
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
