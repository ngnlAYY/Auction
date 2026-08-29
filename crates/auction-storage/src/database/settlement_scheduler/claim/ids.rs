//! 结算 workflow 的稳定身份和命令请求摘要。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use auction_application::RequestHash;
use auction_domain::CommandId;

/// 结算领取命令写入账本时使用的稳定请求内容。
#[derive(Debug, Serialize)]
pub(super) struct SettlementClaimRequest {
    pub workflow_id: Uuid,
    pub due_at: DateTime<Utc>,
}

pub(super) fn workflow_id(auction_id: Uuid, due_at: DateTime<Utc>) -> Option<Uuid> {
    legacy_worker_id("auction-settlement", "workflow", auction_id, due_at)
}

pub(super) fn claim_command_id(auction_id: Uuid, due_at: DateTime<Utc>) -> Option<CommandId> {
    legacy_worker_id("auction-worker", "settlement_claim", auction_id, due_at)
        .map(CommandId::from_uuid)
}

pub(super) fn settlement_command_id(auction_id: Uuid, due_at: DateTime<Utc>) -> Option<CommandId> {
    legacy_worker_id("auction-worker", "settlement_complete", auction_id, due_at)
        .map(CommandId::from_uuid)
}

pub(super) fn request_hash(
    auction_id: Uuid,
    workflow_id: Uuid,
    due_at: DateTime<Utc>,
) -> Result<Vec<u8>, serde_json::Error> {
    RequestHash::from_request(
        "claim_settlement",
        auction_id,
        &SettlementClaimRequest { workflow_id, due_at },
    )
    .map(|hash| hash.as_bytes().to_vec())
}

/// 逐字复现旧 Worker 的长度前缀 SHA-256 身份，due 世代必须参与 workflow 和两个命令 ID。
fn legacy_worker_id(
    namespace: &str,
    kind: &str,
    auction_id: Uuid,
    due_at: DateTime<Utc>,
) -> Option<Uuid> {
    let mut hasher = Sha256::new();
    for value in [namespace.as_bytes(), kind.as_bytes(), auction_id.as_bytes()] {
        hasher.update(u64::try_from(value.len()).ok()?.to_be_bytes());
        hasher.update(value);
    }
    let due_nanos = due_at.timestamp_nanos_opt()?;
    hasher.update((due_nanos as u64).to_be_bytes());
    let mut bytes: [u8; 16] = hasher.finalize()[..16].try_into().ok()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Some(Uuid::from_bytes(bytes))
}
