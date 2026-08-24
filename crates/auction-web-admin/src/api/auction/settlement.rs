//! Admin 结算报告 HTTP 客户端。
//!
//! 结算报告只读取后端的安全投影；浏览器不接触数据库结算记录，也不在客户端重新计算
//! 买卖双方金额。金额和资金池摘要必须以同一份服务端快照为准。

pub(crate) use auction_contracts::{
    AdminSettlementLotResponse as AdminSettlementLot,
    AdminSettlementReportResponse as AdminSettlementReport,
    AdminSettlementSummaryResponse as AdminSettlementSummary,
};
use gloo_net::http::Request;

use super::ApiResponse;

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
