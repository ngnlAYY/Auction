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
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
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
