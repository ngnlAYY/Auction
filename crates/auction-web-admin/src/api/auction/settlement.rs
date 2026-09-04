//! Admin 结算报告 HTTP 客户端。
//!
//! 结算报告只读取后端的安全投影；浏览器不接触数据库结算记录，也不在客户端重新计算
//! 买卖双方金额。金额和资金池摘要必须以同一份服务端快照为准。

use auction_contracts::{AdminSettlementAdjustmentResponse, AdminSettlementAuctionResponse};
pub(crate) use auction_contracts::{
    AdminSettlementLotResponse as AdminSettlementLot,
    AdminSettlementReportResponse as AdminSettlementReport,
    AdminSettlementSummaryResponse as AdminSettlementSummary,
};
use serde::Deserialize;

use super::auctions::{StrictAuction, decode_auction};
use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, OptionalField, RequiredOption, invalid_response_error,
    response_error, send_get, valid_admin_time, valid_trimmed_text,
};

/// 旧结算 decoder 只对内嵌 auction 使用字段闭集；report、summary 与 lot 允许兼容扩展字段。
#[derive(Debug, Deserialize)]
struct StrictSettlementReport {
    auction: StrictAuction,
    lots: Vec<StrictSettlementLot>,
    settlement: RequiredOption<StrictSettlementSummary>,
}

#[derive(Debug, Deserialize)]
struct StrictSettlementSummary {
    closing_balance: i64,
    compensation_out: i64,
    discount_demand: i64,
    movement_count: i64,
    opening_balance: i64,
    settled_at: String,
    tax_income: i64,
}

#[derive(Debug, Deserialize)]
struct StrictSettlementLot {
    adjustment: StrictSettlementAdjustment,
    buyer_amount: i64,
    compensation_amount: i64,
    equipment_category: String,
    equipment_name: String,
    final_price: i64,
    listed_price: i64,
    hold_ppm: i32,
    adjustment_ppm: i32,
    id: String,
    lot_number: i32,
    seller: RequiredOption<String>,
    seller_amount: i64,
    status: String,
    tax_amount: i64,
    winner: RequiredOption<String>,
}

#[derive(Debug, Deserialize)]
struct StrictSettlementAdjustment {
    #[serde(default)]
    policy: OptionalField<String>,
}

/// 读取指定拍卖的结算报告。
pub(crate) async fn fetch_admin_settlement(
    auction_id: String,
) -> Result<AdminSettlementReport, String> {
    let url = format!("/api/v1/admin/auctions/{auction_id}/settlement-report");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictSettlementReport> =
        response.json().await.map_err(invalid_response_error)?;
    let report = decode_settlement_report(payload.data).map_err(invalid_response_error)?;
    if report.auction.id != auction_id || report.auction.status != "settled" {
        return Err(invalid_response_error("settlement_report_owner"));
    }
    Ok(report)
}

fn decode_settlement_report(
    report: StrictSettlementReport,
) -> Result<AdminSettlementReport, &'static str> {
    let auction = settlement_auction(decode_auction(report.auction)?);
    let settlement = report.settlement.0.map(decode_settlement).transpose()?;
    let lots = report.lots.into_iter().map(decode_settlement_lot).collect::<Result<Vec<_>, _>>()?;
    Ok(AdminSettlementReport { auction, settlement, lots })
}

fn decode_settlement(
    settlement: StrictSettlementSummary,
) -> Result<AdminSettlementSummary, &'static str> {
    if !valid_count(settlement.opening_balance)
        || !valid_count(settlement.tax_income)
        || !valid_count(settlement.discount_demand)
        || !valid_count(settlement.compensation_out)
        || !valid_count(settlement.closing_balance)
        || !valid_count(settlement.movement_count)
        || !valid_admin_time(&settlement.settled_at)
    {
        return Err("settlement_summary");
    }
    Ok(AdminSettlementSummary {
        opening_balance: settlement.opening_balance,
        tax_income: settlement.tax_income,
        discount_demand: settlement.discount_demand,
        compensation_out: settlement.compensation_out,
        closing_balance: settlement.closing_balance,
        movement_count: settlement.movement_count,
        settled_at: settlement.settled_at,
    })
}

fn decode_settlement_lot(lot: StrictSettlementLot) -> Result<AdminSettlementLot, &'static str> {
    if !super::valid_uuid(&lot.id)
        || lot.lot_number <= 0
        || !valid_trimmed_text(&lot.status)
        || !valid_trimmed_text(&lot.equipment_name)
        || !valid_trimmed_text(&lot.equipment_category)
        || !valid_optional_text(lot.seller.0.as_deref())
        || !valid_optional_text(lot.winner.0.as_deref())
        || !valid_count(lot.final_price)
        || !valid_count(lot.listed_price)
        || !valid_count(lot.buyer_amount)
        || !valid_count(lot.seller_amount)
        || !valid_count(lot.tax_amount)
        || !valid_count(lot.compensation_amount)
        || !(0..=1_000_000).contains(&lot.hold_ppm)
        || !(-1_000_000..=1_000_000).contains(&lot.adjustment_ppm)
        || !valid_optional_text(lot.adjustment.policy.0.as_deref())
    {
        return Err("settlement_lot");
    }
    Ok(AdminSettlementLot {
        id: lot.id,
        lot_number: lot.lot_number,
        status: lot.status,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        seller: lot.seller.0,
        winner: lot.winner.0,
        final_price: lot.final_price,
        listed_price: lot.listed_price,
        buyer_amount: lot.buyer_amount,
        seller_amount: lot.seller_amount,
        tax_amount: lot.tax_amount,
        compensation_amount: lot.compensation_amount,
        hold_ppm: lot.hold_ppm,
        adjustment_ppm: lot.adjustment_ppm,
        adjustment: AdminSettlementAdjustmentResponse { policy: lot.adjustment.policy.0 },
    })
}

fn settlement_auction(
    auction: auction_contracts::AdminAuctionResponse,
) -> AdminSettlementAuctionResponse {
    AdminSettlementAuctionResponse {
        id: auction.id,
        world_id: auction.world_id,
        sequence: auction.sequence,
        status: auction.status,
        preview_scheduled_at: auction.preview_scheduled_at,
        scheduled_open_at: auction.scheduled_open_at,
        preview_opened_at: auction.preview_opened_at,
        preview_manual_open: auction.preview_manual_open,
        preview_revision: auction.preview_revision,
        opened_at: auction.opened_at,
        final_phase_at: auction.final_phase_at,
        settled_at: auction.settled_at,
        cancelled_at: auction.cancelled_at,
        cancelled_reason: auction.cancelled_reason,
        cancelled_by: auction.cancelled_by,
        state_version: auction.state_version,
        created_at: auction.created_at,
        updated_at: auction.updated_at,
    }
}

fn valid_count(value: i64) -> bool {
    (0..=MAX_JSON_SAFE_INTEGER).contains(&value)
}

fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_trimmed_text)
}
