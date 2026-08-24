//! auction 相关 DTO。

use super::*;

/// Admin 拍卖生命周期命令的稳定结果投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionLifecycleResponse {
    pub command_id: Uuid,
    pub auction_id: String,
    pub status: String,
    pub state_version: u64,
    pub updated_at: String,
}

/// Admin 创建 draft 拍卖请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCreateAuctionRequest {
    pub command_id: Uuid,
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
    pub preview_scheduled_at: DateTime<Utc>,
    pub scheduled_open_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
    pub expected_state_version: i64,
}

/// Admin 创建拍卖成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminAuctionCreateResponse {
    pub auction: AdminAuctionResponse,
}

/// Admin 装备录入请求。
///
/// `source_kind` 缺省时沿用旧契约的 URL 来源；URL 与 canonical JSON 必须二选一。装备 key
/// 只在 HTTPS 请求和应用适配器的短生命周期内使用，绝不进入成功响应。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminIntakeRequest {
    pub command_id: Uuid,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub equipment_url: Option<String>,
    #[serde(default)]
    pub equipment: Option<AdminCanonicalEquipmentRequest>,
    pub start_price: i64,
    pub player_id: String,
}

/// Admin canonical 装备输入 DTO；HTTP 边界保持字段闭集，避免把任意 JSON 透传到来源适配器。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentRequest {
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub equipment_type: String,
    pub quality: String,
    pub prefix: String,
    pub suffix: String,
    #[serde(default)]
    pub level: Option<i32>,
    pub tradeable: bool,
    pub stats: BTreeMap<String, AdminCanonicalEquipmentStatRequest>,
    pub key: String,
}

/// canonical 装备属性 DTO。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminCanonicalEquipmentStatRequest {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
}

impl std::fmt::Debug for AdminIntakeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminIntakeRequest")
            .field("command_id", &self.command_id)
            .field("source_kind", &self.source_kind)
            .field("equipment_url", &self.equipment_url)
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
        let mut bytes = std::mem::take(&mut self.key).into_bytes();
        bytes.fill(0);
    }
}

/// Admin 装备录入成功响应；字段与旧版 `addIntakeItem` 契约保持一致。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
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
    pub equipment_level: Option<i32>,
    pub start_price: i64,
    pub seller_player_id: String,
}
