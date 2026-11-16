//! Public 履约进度 DTO：已结算场次逐件的结算后流程状态。
//!
//! 字段闭集首发锁定；以后新增字段只能经可选查询参数或新版本路由（见 README 的变更规则）。
//! 不含邮件、投递、履约或命令标识、诊断码、Bot 身份与代理模式。

use serde::{Deserialize, Serialize};

/// 履约进度所属的已结算场次。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFulfillmentAuctionResponse {
    pub id: String,
    pub world: String,
    /// 场次期号（`auctions.sequence`）。
    pub number: i64,
}

/// 单件 lot 的公开履约状态。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFulfillmentLotResponse {
    pub lot_id: String,
    /// 类别内编号，与竞价日志、补偿池明细同一口径。
    pub lot_number: i32,
    pub equipment_name: String,
    pub equipment_category: String,
    /// 投稿卖家名；没有投稿记录的历史 lot 为 `—`。
    pub seller_name: String,
    /// 成交 lot 的买家当前用户名，与 `final_price` 成对出现；流拍不输出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buyer_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_price: Option<i64>,
    /// 闭集状态，取值与含义见 docs/contracts/routes.md。
    pub status: String,
    /// 当前付款期限（RFC 3339 UTC，秒精度）；只在等待付款、已逾期与重寄后等待付款时输出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_deadline_at: Option<String>,
    /// 发生过 COD 退回时与 `cod_return_limit` 成对输出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cod_return_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cod_return_limit: Option<i32>,
}

/// 履约进度编号页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicFulfillmentPageResponse {
    pub auction: PublicFulfillmentAuctionResponse,
    pub lots: Vec<PublicFulfillmentLotResponse>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}
