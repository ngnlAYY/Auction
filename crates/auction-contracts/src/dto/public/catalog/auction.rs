//! Public 拍卖历史列表和当前拍卖 DTO。

use serde::{Deserialize, Serialize};

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

/// 按 ID 读取的已结算场次摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SettledAuctionSummaryResponse {
    pub id: String,
    pub world: String,
    pub sequence: i64,
    /// 与目录使用同一截止时刻；没有有效截止时刻时返回 null。
    #[serde(deserialize_with = "crate::dto::response::deserialize_required_option")]
    pub effective_end_at: Option<String>,
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
    /// 该场冻结策略的代理默认模式（`fast`|`slow`），Public 代理新建表单以它为初始选项；代理
    /// 授权接口仍要求显式 `mode`。服务端只在请求带 `include=proxy_default_mode` 时输出，缺省
    /// 响应省略该键，保持旧页面按闭集解码的原字段集合；浏览器按往期目录拼装的已结算场次没有该值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_default_mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::SettledAuctionSummaryResponse;
    use crate::ApiResponse;

    #[test]
    fn settled_auction_summary_has_a_closed_field_set() {
        let mut wire: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/public/auction-summary.json"
        ))
        .unwrap();
        let parsed: ApiResponse<SettledAuctionSummaryResponse> =
            serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(parsed.data.effective_end_at.as_deref(), Some("2025-11-17T09:46:40Z"));
        wire["data"]["effective_end_at"] = Value::Null;
        let parsed: ApiResponse<SettledAuctionSummaryResponse> =
            serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(parsed.data.effective_end_at, None);

        let mut extra = wire.clone();
        extra["data"]["status"] = Value::String("settled".to_owned());
        assert!(
            serde_json::from_value::<ApiResponse<SettledAuctionSummaryResponse>>(extra).is_err()
        );
        wire["data"].as_object_mut().unwrap().remove("effective_end_at");
        assert!(
            serde_json::from_value::<ApiResponse<SettledAuctionSummaryResponse>>(wire).is_err()
        );
    }
}
