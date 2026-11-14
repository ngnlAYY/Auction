//! auction 相关 DTO。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::shared::{deserialize_null_default, deserialize_required_option};
use super::AdminAuctionResponse;

/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；wire 冻结后只收紧
/// 反序列化，序列化输出不变。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionLifecycleResponse {
    pub auction: AdminAuctionCommandResponse,
}

/// 旧版生命周期响应不会附带列表专属的 `scheduled_ends_at` 或 `lot_count`，因此这里不复用
/// `AdminAuctionResponse`，避免把列表扩展字段泄漏进同一 v1 命令 wire。nullable 生命周期字段
/// 在旧 decoder 中必须显式出现（可为 `null`），未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionCommandResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub opened_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub final_phase_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub settled_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_reason: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCreateAuctionRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub auction_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub world_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub sequence: i64,
    #[serde(default)]
    pub preview_scheduled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scheduled_open_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scheduled_end_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionCreateResponse {
    pub auction: AdminAuctionCommandResponse,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminUpdateAuctionScheduleRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default)]
    pub preview_scheduled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scheduled_open_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scheduled_end_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub expected_state_version: i64,
}

/// Admin 拍卖计划修改成功响应；返回列表同形投影以显式携带三项计划时间。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminUpdateAuctionScheduleResponse {
    pub auction: AdminAuctionResponse,
}

/// `source_kind` 缺省时沿用旧契约的 URL 来源；URL 与 canonical JSON 必须二选一。装备 key
/// 只在 HTTPS 请求和应用适配器的短生命周期内使用，绝不进入成功响应。
/// 完整来源 URL 同样包含装备 key；调试输出不能只遮蔽 canonical JSON 中的独立 key 字段。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminIntakeRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub equipment_url: Option<String>,
    #[serde(default)]
    pub equipment: Option<AdminCanonicalEquipmentRequest>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub start_price: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub player_id: String,
}

/// Admin canonical 装备输入 DTO。
/// 旧版使用 pointer-heavy wire 区分“字段缺失/`null`”和合法零值；因此除 `level` 外的必填字段
/// 在 DTO 中保留 `Option`，由 HTTP 防腐层统一映射为 `invalid_auction_command`。严格字段闭集仍由
/// `deny_unknown_fields` 保证。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentRequest {
    #[serde(default)]
    pub external_equipment_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// 历史 canonical 基础名，护甲仍为种类与部位的组合（如 `Phase Robe`）。
    #[serde(default)]
    pub equipment_type: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub level: Option<i32>,
    #[serde(default)]
    pub tradeable: Option<bool>,
    #[serde(default)]
    pub stats: Option<BTreeMap<String, AdminCanonicalEquipmentStatRequest>>,
    #[serde(default)]
    pub key: Option<String>,
}

/// canonical 装备属性 DTO。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentStatRequest {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
}

impl std::fmt::Debug for AdminIntakeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminIntakeRequest")
            .field("command_id", &self.command_id)
            .field("source_kind", &self.source_kind)
            .field("equipment_url", &self.equipment_url.as_ref().map(|_| "[REDACTED]"))
            .field("equipment", &self.equipment.as_ref().map(|_| "[REDACTED]"))
            .field("start_price", &self.start_price)
            .field("player_id", &self.player_id)
            .finish()
    }
}

impl std::fmt::Debug for AdminCanonicalEquipmentRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCanonicalEquipmentRequest")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("equipment_type", &self.equipment_type)
            .field("quality", &self.quality)
            .field("prefix", &self.prefix)
            .field("suffix", &self.suffix)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("stats", &self.stats)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for AdminCanonicalEquipmentStatRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminCanonicalEquipmentStatRequest")
            .field("value", &self.value)
            .field("base", &self.base)
            .finish()
    }
}

impl Drop for AdminCanonicalEquipmentRequest {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let mut bytes = key.into_bytes();
            bytes.fill(0);
        }
    }
}

/// Admin 装备录入成功响应；字段与旧版 `addIntakeItem` 契约保持一致。nullable 的
/// `equipment_level` 在旧 decoder 中必须显式出现（可为 `null`），未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminIntakeResponse {
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
    #[serde(deserialize_with = "deserialize_required_option")]
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}
