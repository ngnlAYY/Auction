//! Public 补偿池及账本 DTO。

use super::*;

/// Public 补偿池批次的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationRoundResponse {
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

/// Public 补偿池账本 movement 的稳定响应。
/// 完整游戏装备 URL 的 key 只允许进入 wire，调试输出始终脱敏。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationMovementResponse {
    pub sequence: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_equipment_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_ppm: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjustment_ppm: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_held_nanos: Option<i64>,
    pub kind: String,
    pub amount: i64,
    pub balance_after: i64,
    pub created_at: String,
}

impl std::fmt::Debug for PublicCompensationMovementResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicCompensationMovementResponse")
            .field("sequence", &self.sequence)
            .field("lot_id", &self.lot_id)
            .field("lot_number", &self.lot_number)
            .field("equipment_name", &self.equipment_name)
            .field("equipment_category", &self.equipment_category)
            .field("game_equipment_url", &self.game_equipment_url.as_ref().map(|_| "[REDACTED]"))
            .field("hold_ppm", &self.hold_ppm)
            .field("adjustment_ppm", &self.adjustment_ppm)
            .field("leader_held_nanos", &self.leader_held_nanos)
            .field("kind", &self.kind)
            .field("amount", &self.amount)
            .field("balance_after", &self.balance_after)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// Public 补偿池批次列表响应；编号页与 cursor 页字段互斥。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationResponse {
    pub balance: i64,
    pub updated_at: Option<String>,
    pub rounds: Vec<PublicCompensationRoundResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationMovementPageResponse {
    pub round: PublicCompensationRoundResponse,
    pub movements: Vec<PublicCompensationMovementResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
