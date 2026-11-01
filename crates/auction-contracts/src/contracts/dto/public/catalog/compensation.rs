//! Public 补偿池及账本 DTO。

use super::*;

/// Public 补偿池批次的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationRoundResponse {
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    /// 该场冻结的结算算法：`legacy_zero_v0`、`hold_curve_v1` 或 `hold_curve_v2`。
    pub policy_mode: String,
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    /// 本场成交撤销后写入的冲正笔数。
    pub reversal_count: i64,
    /// 本场冲正对池余额的带符号净变化。
    pub reversal_amount: i64,
    pub settled_at: String,
}

/// 单个世界补偿池的当前状态。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationPoolResponse {
    pub world_id: String,
    pub balance: i64,
    pub updated_at: String,
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
    /// 各世界补偿池余额合计；各池独立结算，按世界的余额见 `pools`。
    pub balance: i64,
    pub updated_at: Option<String>,
    pub pools: Vec<PublicCompensationPoolResponse>,
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

/// 成交撤销后对补偿池的冲正。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationLotReversalResponse {
    /// 对池余额的带符号净变化：退回补偿减去冲回附加费。
    pub amount: i64,
    /// 池余额不足、未能冲回的附加费。
    pub tax_shortfall: i64,
    pub balance_after: i64,
    pub reversed_at: String,
}

/// 单个已售 lot 的公开结算明细。
/// 完整游戏装备 URL 的 key 只允许进入 wire，调试输出始终脱敏。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationLotResponse {
    pub lot_id: String,
    pub lot_number: i32,
    pub equipment_name: String,
    pub equipment_category: String,
    pub game_equipment_url: String,
    pub end_price: i64,
    pub hold_ppm: i32,
    pub adjustment_ppm: i32,
    pub leader_held_nanos: i64,
    pub tax_amount: i64,
    /// 名义优惠需求：按调整比例计算、尚未按池余额缩放的优惠。
    pub discount_amount: i64,
    pub compensation_amount: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    /// 成交是否已被撤销；升级前撤销的成交可能没有冲正。
    pub voided: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversal: Option<PublicCompensationLotReversalResponse>,
}

impl std::fmt::Debug for PublicCompensationLotResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicCompensationLotResponse")
            .field("lot_id", &self.lot_id)
            .field("lot_number", &self.lot_number)
            .field("equipment_name", &self.equipment_name)
            .field("equipment_category", &self.equipment_category)
            .field("game_equipment_url", &"[REDACTED]")
            .field("end_price", &self.end_price)
            .field("hold_ppm", &self.hold_ppm)
            .field("adjustment_ppm", &self.adjustment_ppm)
            .field("leader_held_nanos", &self.leader_held_nanos)
            .field("tax_amount", &self.tax_amount)
            .field("discount_amount", &self.discount_amount)
            .field("compensation_amount", &self.compensation_amount)
            .field("buyer_amount", &self.buyer_amount)
            .field("seller_amount", &self.seller_amount)
            .field("voided", &self.voided)
            .field("reversal", &self.reversal)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationLotPageResponse {
    pub round: PublicCompensationRoundResponse,
    pub lots: Vec<PublicCompensationLotResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
