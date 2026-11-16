//! Public 竞价日志和价格历史 DTO。

use serde::{Deserialize, Serialize};

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
    /// 仅在论坛主题、帖子与楼层均可证明时返回的真实帖子链接。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_post_url: Option<String>,
    pub valid: bool,
    pub accepted_state_version: i64,
    pub fact_index: i64,
    pub mechanism: String,
    pub created_at: String,
    /// 本条代理出价在代理对代理竞争中击败的授权；只在 `include=defeated_proxy` 时输出，页面在
    /// 本条之前按一次代理出价展示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defeated_proxy: Option<PublicDefeatedProxyResponse>,
}

/// 竞价日志中落败代理的公开投影：只有出价人与落败时的上限，不含模式。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicDefeatedProxyResponse {
    pub bidder_name: String,
    pub maximum_amount: i64,
}

/// Public 竞价日志列表响应；编号页与 cursor 页使用已发布的互斥字段形状。
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
    /// 精确中位数；规范十进制整数或 .5，缺失时沿用旧整数 median。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_exact: Option<String>,
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
