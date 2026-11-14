//! 进入事务的脱敏录入载荷、结果和审计记录。

use auction_domain::{CommandId, EquipmentId, EquipmentKeyEnvelope};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::common::{AdminIntakeError, decimal_identity, valid_text};
use super::equipment::AdminIntakeSourceKind;
use crate::ports::core::RequestHash;

/// 进入 owner 事务的脱敏事实集合。
#[derive(Clone)]
pub struct AdminIntakeWrite {
    pub command_id: CommandId,
    pub aggregate_id: Uuid,
    pub world_id: String,
    pub source_kind: AdminIntakeSourceKind,
    pub source_reference: String,
    pub source_fingerprint: [u8; 32],
    pub actor_id: String,
    pub request_id: Option<String>,
    pub player_id: String,
    pub start_price: i64,
    pub candidate_equipment_id: EquipmentId,
    pub candidate_submission_id: Uuid,
    pub candidate_lot_id: Uuid,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub equipment_attributes: Value,
    pub collected_at: DateTime<Utc>,
    pub key_envelope: EquipmentKeyEnvelope,
    pub request_hash: RequestHash,
}

impl AdminIntakeWrite {
    /// 校验写入结构不含 raw source/key，并确保候选标识互不相同。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.command_id.is_nil()
            || self.aggregate_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || !valid_text(&self.source_reference, 512)
            || self.source_fingerprint == [0; 32]
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
            || !valid_text(&self.player_id, 256)
            || self.start_price <= 0
            || self.candidate_equipment_id.is_nil()
            || self.candidate_submission_id.is_nil()
            || self.candidate_lot_id.is_nil()
            || self.candidate_equipment_id.as_uuid() == self.candidate_submission_id
            || self.candidate_equipment_id.as_uuid() == self.candidate_lot_id
            || self.candidate_submission_id == self.candidate_lot_id
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.equipment_name, 512)
            || !valid_text(&self.equipment_category, 128)
            || self.equipment_level.is_some_and(|level| !(0..=100_000).contains(&level))
            || !self.equipment_attributes.is_object()
            || self.collected_at.timestamp_micros() == 0
            || self.key_envelope.validate().is_err()
        {
            return Err(AdminIntakeError::InvalidSource);
        }
        Ok(())
    }
}

/// Admin 录入完成后可安全重放的结果。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminIntakeResult {
    pub auction_id: Uuid,
    pub auction_sequence: i64,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub submission_id: Uuid,
    pub lot_id: Uuid,
    pub lot_number: i32,
    pub lot_state_version: i64,
    pub equipment_id: Uuid,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}

impl AdminIntakeResult {
    /// 校验命令账本中恢复出的安全结果。
    pub fn validate(&self) -> Result<(), AdminIntakeError> {
        if self.auction_id.is_nil()
            || self.auction_sequence <= 0
            || self.auction_state_version <= 0
            || self.preview_revision < 0
            || self.submission_id.is_nil()
            || self.lot_id.is_nil()
            || self.lot_number <= 0
            || self.lot_state_version <= 0
            || self.equipment_id.is_nil()
            || !decimal_identity(&self.external_equipment_id)
            || !valid_text(&self.equipment_name, 512)
            || !valid_text(&self.equipment_category, 128)
            || self.equipment_level.is_some_and(|level| !(0..=100_000).contains(&level))
            || self.start_price <= 0
            || !valid_text(&self.seller_player_id, 256)
        {
            return Err(AdminIntakeError::StoredResultCorrupt);
        }
        Ok(())
    }
}

/// 进入同一事务的安全审计信息。
#[derive(Debug, Clone)]
pub struct AdminIntakeRecord {
    pub command_id: CommandId,
    pub world_id: String,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub result: AdminIntakeResult,
}
