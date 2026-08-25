//! 结算 workflow 的稳定身份和命令请求摘要。

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use auction_application::RequestHash;
use auction_domain::{CommandId, deterministic_uuid};

/// 结算领取命令写入账本时使用的稳定请求内容。
#[derive(Debug, Serialize)]
pub(super) struct SettlementClaimRequest {
    pub workflow_id: Uuid,
    pub due_at: DateTime<Utc>,
}

pub(super) fn workflow_id(auction_id: Uuid) -> Uuid {
    deterministic_uuid(CommandId::from_uuid(auction_id), "settlement-workflow", 1)
}

pub(super) fn claim_command_id(auction_id: Uuid) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(auction_id),
        "settlement-claim",
        1,
    ))
}

pub(super) fn settlement_command_id(claim_command_id: CommandId) -> CommandId {
    CommandId::from_uuid(deterministic_uuid(claim_command_id, "settlement-execute", 1))
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
