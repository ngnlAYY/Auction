//! 旧版 `settlement_claim` 与新版领取账本的局部兼容校验。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use auction_domain::CommandId;

use super::ids::request_hash;

const LEGACY_SETTLEMENT_ACTOR: &str = "auction-worker";

pub(super) struct ClaimLedgerRecord {
    pub command_kind: String,
    pub aggregate_id: Uuid,
    pub request_hash: Vec<u8>,
    pub result_code: String,
    pub result_payload: Value,
}

pub(super) enum ClaimLedgerValidationError {
    Conflict,
    Corrupt,
}

#[derive(Serialize)]
struct LegacySettlementClaimHash<'a> {
    #[serde(rename = "CommandID")]
    command_id: [u8; 16],
    #[serde(rename = "WorkflowID")]
    workflow_id: [u8; 16],
    #[serde(rename = "AuctionID")]
    auction_id: [u8; 16],
    #[serde(rename = "DueAt")]
    due_at: &'a str,
    #[serde(rename = "Actor")]
    actor: &'static str,
    #[serde(rename = "Now")]
    now: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentSettlementClaimResult {
    auction_id: Uuid,
    workflow_id: Uuid,
    due_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySettlementClaimResult {
    auction_id: [u8; 16],
    workflow_id: [u8; 16],
    command_id: [u8; 16],
    due_at: DateTime<Utc>,
    started_at: DateTime<Utc>,
    attempt_count: i64,
    state_version: i64,
}

/// 校验领取命令账本；旧格式只允许在已进入 settling/settled 的接管路径重放。
pub(super) fn validate_claim_ledger(
    record: ClaimLedgerRecord,
    auction_id: Uuid,
    workflow_id: Uuid,
    claim_command_id: CommandId,
    due_at: DateTime<Utc>,
    allow_legacy: bool,
) -> Result<(), ClaimLedgerValidationError> {
    let current_hash = request_hash(auction_id, workflow_id, due_at)
        .map_err(|_| ClaimLedgerValidationError::Corrupt)?;
    let current_request = record.command_kind == "claim_settlement"
        && record.aggregate_id == auction_id
        && record.request_hash == current_hash;
    if current_request {
        if record.result_code != "accepted" {
            return Err(ClaimLedgerValidationError::Corrupt);
        }
        let result: CurrentSettlementClaimResult = serde_json::from_value(record.result_payload)
            .map_err(|_| ClaimLedgerValidationError::Corrupt)?;
        if result.auction_id != auction_id
            || result.workflow_id != workflow_id
            || result.due_at != due_at
        {
            return Err(ClaimLedgerValidationError::Corrupt);
        }
        return Ok(());
    }

    let legacy_hash = legacy_claim_hash(auction_id, workflow_id, claim_command_id, due_at)
        .map_err(|_| ClaimLedgerValidationError::Corrupt)?;
    let legacy_request = record.command_kind == "settlement_claim"
        && record.aggregate_id == auction_id
        && record.request_hash == legacy_hash;
    if !legacy_request || !allow_legacy {
        return Err(ClaimLedgerValidationError::Conflict);
    }
    if record.result_code != "OK" {
        return Err(ClaimLedgerValidationError::Corrupt);
    }
    let result: LegacySettlementClaimResult = serde_json::from_value(record.result_payload)
        .map_err(|_| ClaimLedgerValidationError::Corrupt)?;
    if result.auction_id != uuid_bytes(auction_id)
        || result.workflow_id != uuid_bytes(workflow_id)
        || result.command_id != uuid_bytes(claim_command_id.as_uuid())
        || result.due_at != due_at
        || result.started_at < due_at
        || result.started_at.timestamp_nanos_opt().is_none()
        || result.attempt_count <= 0
        || result.state_version <= 0
    {
        return Err(ClaimLedgerValidationError::Corrupt);
    }
    Ok(())
}

fn legacy_claim_hash(
    auction_id: Uuid,
    workflow_id: Uuid,
    claim_command_id: CommandId,
    due_at: DateTime<Utc>,
) -> Result<Vec<u8>, serde_json::Error> {
    let due_at = due_at.to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let encoded = serde_json::to_vec(&LegacySettlementClaimHash {
        command_id: uuid_bytes(claim_command_id.as_uuid()),
        workflow_id: uuid_bytes(workflow_id),
        auction_id: uuid_bytes(auction_id),
        due_at: &due_at,
        actor: LEGACY_SETTLEMENT_ACTOR,
        now: &due_at,
    })?;
    Ok(Sha256::digest(encoded).to_vec())
}

fn uuid_bytes(value: Uuid) -> [u8; 16] {
    *value.as_bytes()
}
