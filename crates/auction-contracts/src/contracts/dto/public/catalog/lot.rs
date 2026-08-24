//! Public 正式/预览 lot、装备和响应外壳 DTO。

use super::*;

/// lot 列表与详情共用的公开拍卖上下文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLotProjectionResponse {
    pub auction_id: String,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_ends_at: Option<String>,
    pub preview_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_restriction: Option<PublicFinalRestrictionResponse>,
}

/// 终阶段代理限制的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFinalRestrictionResponse {
    pub new_slow_proxy_allowed: bool,
    pub existing_slow_proxy_converted: bool,
}

/// 公开装备 stat 的稳定 JSON 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentStatResponse {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f64>,
}

/// 公开装备详情；attributes 仅由 lot 详情显式传入。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicEquipmentResponse {
    pub name: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub info_summary: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub stats: BTreeMap<String, PublicEquipmentStatResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

/// 正式竞拍 lot 的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLiveLotResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_number: i64,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub next_bid: i64,
    pub has_leader: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_name: Option<String>,
    pub seller_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_end_at: Option<String>,
    pub state_version: i64,
    pub equipment: PublicEquipmentResponse,
}

/// 预览 lot 的公开契约。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotResponse {
    pub id: String,
    pub auction_id: String,
    pub code: String,
    pub status: String,
    pub seller_name: String,
    pub start_price: i64,
    pub equipment_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub equipment_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_roll: Option<f64>,
    pub game_equipment_url: String,
}

/// 预览 lot 详情沿用摘要字段，并附加完整公开装备投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPreviewLotDetailResponse {
    #[serde(flatten)]
    pub summary: PublicPreviewLotResponse,
    pub equipment: PublicEquipmentResponse,
}

/// lot 列表的无标签数组，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotListItemsResponse {
    Preview(Vec<PublicPreviewLotResponse>),
    Live(Vec<PublicLiveLotResponse>),
}

/// lot 详情的无标签对象，保持旧 JSON 契约的 preview/live 形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PublicLotDetailItemResponse {
    Preview(PublicPreviewLotDetailResponse),
    Live(PublicLiveLotResponse),
}

/// Public lot 列表与详情共用的响应外壳。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLotEnvelopeResponse {
    pub auction_id: String,
    pub world: String,
    pub number: i64,
    pub auction_status: String,
    pub projection_kind: String,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub opens_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_ends_at: Option<String>,
    pub preview_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_restriction: Option<PublicFinalRestrictionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lots: Option<PublicLotListItemsResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lot: Option<PublicLotDetailItemResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_counts: Option<BTreeMap<String, i64>>,
}
