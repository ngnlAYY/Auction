//! 结算领取命令的稳定请求摘要。

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use auction_application::RequestHash;

/// 结算领取命令写入账本时使用的稳定请求内容。
#[derive(Debug, Serialize)]
pub(super) struct SettlementClaimRequest {
    pub workflow_id: Uuid,
    pub due_at: DateTime<Utc>,
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
