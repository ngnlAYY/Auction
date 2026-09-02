//! Public 竞价日志和价格历史 DTO。

use super::*;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_post_number: Option<i64>,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i64,
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
