//! catalog 相关 DTO。

use super::*;

/// Public 拍卖历史列表中的稳定摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionSummaryResponse {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub scheduled_open_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_phase_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_end_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    pub lot_count: i64,
    pub sold_count: i64,
    pub unsold_count: i64,
    pub sold_amount: i64,
    pub state_version: i64,
}

/// Public 历史拍卖列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionListResponse {
    pub auctions: Vec<AuctionSummaryResponse>,
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

/// Public 竞价日志条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBidLogEntryResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub bidder_name: String,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: String,
}

/// Public 竞价日志列表响应；编号页与 cursor 页沿用旧系统互斥字段形状。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBidLogListResponse {
    pub bids: Vec<PublicBidLogEntryResponse>,
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

/// Public 价格历史条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryEntryResponse {
    pub lot_id: String,
    pub auction_id: String,
    pub world_id: String,
    pub auction_sequence: i64,
    pub lot_number: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equipment_level: Option<i64>,
    pub buyer_name: String,
    pub seller_name: String,
    pub price: i64,
    pub closed_at: String,
}

/// Public 价格历史编号页的完整过滤集统计。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryStatsResponse {
    pub count: i64,
    pub min: i64,
    pub max: i64,
    pub median: i64,
}

/// Public 价格历史响应；cursor 页不携带 stats/total。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicPriceHistoryListResponse {
    pub rows: Vec<PublicPriceHistoryEntryResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<PublicPriceHistoryStatsResponse>,
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

/// Public 单场补偿账本响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicCompensationMovementPageResponse {
    pub round: PublicCompensationRoundResponse,
    pub movements: Vec<PublicCompensationMovementResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

/// Public 论坛镜像同步状态响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicForumResponse {
    pub auction_id: String,
    pub status: String,
    pub source_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub updated_at: String,
}

/// 当前拍卖中的公开上下文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CurrentAuctionResponse {
    pub auction: Option<CurrentAuctionItemResponse>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CurrentAuctionItemResponse {
    pub id: String,
    pub world: String,
    pub number: i64,
    pub status: String,
    pub opens_at: String,
    pub scheduled_ends_at: Option<String>,
    pub presentation_mode: String,
    pub bidding_open: bool,
    pub preview_revision: i64,
}

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
