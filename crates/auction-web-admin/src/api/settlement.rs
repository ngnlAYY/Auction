//! Admin 结算报告 HTTP 客户端。
//!
//! 结算报告只读取后端的安全投影；浏览器不接触数据库结算记录，也不在客户端重新计算
//! 买卖双方金额。金额和资金池摘要必须以同一份服务端快照为准。

use gloo_net::http::Request;
use serde::Deserialize;

use super::{AdminAuction, ApiResponse};

/// Admin 结算资金池摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminSettlementSummary {
    pub opening_balance: i64,
    pub tax_income: i64,
    pub discount_demand: i64,
    pub compensation_out: i64,
    pub closing_balance: i64,
    pub movement_count: i64,
    pub settled_at: String,
}

/// Admin 结算报告中的 lot 金额投影。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminSettlementLot {
    pub id: String,
    pub lot_number: i32,
    pub status: String,
    pub equipment_name: String,
    pub seller: Option<String>,
    pub winner: Option<String>,
    pub final_price: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
    pub compensation_amount: i64,
    pub adjustment: AdminSettlementAdjustment,
}

/// 结算调整的安全说明。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminSettlementAdjustment {
    pub policy: Option<String>,
}

/// 一个已结算拍卖的完整 Admin 报告。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminSettlementReport {
    pub auction: AdminAuction,
    pub settlement: Option<AdminSettlementSummary>,
    pub lots: Vec<AdminSettlementLot>,
}

/// 读取指定拍卖的结算报告。
pub(crate) async fn fetch_admin_settlement(
    auction_id: String,
) -> Result<AdminSettlementReport, String> {
    let url = format!("/api/v1/admin/auctions/{auction_id}/settlement-report");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminSettlementReport> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
