//! Public 拍卖历史列表和当前拍卖 DTO。

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

/// v2 拍卖摘要：沿用 v1 的平铺字段，论坛地址只在新版本中发布。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionSummaryV2Response {
    #[serde(flatten)]
    pub auction: AuctionSummaryResponse,
    /// 已发布且有内容证明的论坛主题地址；没有主题或尚未对账时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forum_thread_url: Option<String>,
}

/// Public 历史拍卖分页信封；默认条目维持 v1 契约，v2 显式选择扩展摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AuctionListResponse<T = AuctionSummaryResponse> {
    pub auctions: Vec<T>,
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

/// 当前拍卖中的公开上下文。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CurrentAuctionResponse {
    pub auction: Option<CurrentAuctionItemResponse>,
}

/// 当前拍卖条目的稳定响应。
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
