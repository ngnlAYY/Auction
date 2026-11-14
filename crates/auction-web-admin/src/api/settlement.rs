//! Admin 结算报告 HTTP 客户端。
//! 结算报告只读取后端的安全投影；浏览器不接触数据库结算记录，也不在客户端重新计算
//! 买卖双方金额。金额和资金池摘要必须以同一份服务端快照为准。

use auction_contracts::{AdminSettlementAdjustmentResponse, AdminSettlementAuctionResponse};
pub(crate) use auction_contracts::{
    AdminSettlementLotResponse as AdminSettlementLot,
    AdminSettlementReportResponse as AdminSettlementReport,
    AdminSettlementSummaryResponse as AdminSettlementSummary,
};
use serde::Deserialize;

use super::auctions::decode_auction;
use super::{
    ApiResponse, OptionalField, invalid_response_error, response_error, send_get, valid_admin_time,
    valid_count, valid_optional_text, valid_trimmed_text,
};

/// 旧结算 decoder 只对内嵌 auction（由 contracts 闭集 `AdminAuctionResponse` 承担）使用字段闭集；
/// report、summary 与 lot 允许服务端追加字段，contracts 的 `AdminSettlementReportResponse`/
/// `AdminSettlementSummaryResponse`/`AdminSettlementLotResponse`/
/// `AdminSettlementAdjustmentResponse` 不能加 `deny_unknown_fields`，因此保留开放式镜像，
/// 继续要求旧字段显式存在并逐项校验。同步义务：上述 DTO 字段集变化时必须同步本镜像；
/// 下方构造 DTO 的穷举字段列表会在编译期捕捉漂移。
#[derive(Debug, Deserialize)]
struct StrictSettlementReport {
    auction: auction_contracts::AdminAuctionResponse,
    lots: Vec<StrictSettlementLot>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    settlement: Option<StrictSettlementSummary>,
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
    game_equipment_url: String,
    final_price: i64,
    listed_price: i64,
    hold_ppm: i32,
    adjustment_ppm: i32,
    leader_held_nanos: i64,
    id: String,
    lot_number: i32,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    seller: Option<String>,
    seller_amount: i64,
    status: String,
    tax_amount: i64,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    winner: Option<String>,
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
    let settlement = report.settlement.map(decode_settlement).transpose()?;
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

fn valid_equipment_url(value: &str) -> bool {
    ["https://hentaiverse.org/equip/", "https://hentaiverse.org/isekai/equip/"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .is_some_and(|external_id| {
            !external_id.is_empty()
                && external_id.len() <= 32
                && external_id.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn decode_settlement_lot(lot: StrictSettlementLot) -> Result<AdminSettlementLot, &'static str> {
    if !super::valid_uuid(&lot.id)
        || lot.lot_number <= 0
        || !valid_trimmed_text(&lot.status)
        || !valid_trimmed_text(&lot.equipment_name)
        || !valid_trimmed_text(&lot.equipment_category)
        || !valid_equipment_url(&lot.game_equipment_url)
        || !valid_optional_text(lot.seller.as_deref())
        || !valid_optional_text(lot.winner.as_deref())
        || !valid_count(lot.final_price)
        || !valid_count(lot.listed_price)
        || !valid_count(lot.buyer_amount)
        || !valid_count(lot.seller_amount)
        || !valid_count(lot.tax_amount)
        || !valid_count(lot.compensation_amount)
        || !(0..=1_000_000).contains(&lot.hold_ppm)
        || !(-1_000_000..=1_000_000).contains(&lot.adjustment_ppm)
        || lot.leader_held_nanos < 0
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
        game_equipment_url: lot.game_equipment_url,
        seller: lot.seller,
        winner: lot.winner,
        final_price: lot.final_price,
        listed_price: lot.listed_price,
        buyer_amount: lot.buyer_amount,
        seller_amount: lot.seller_amount,
        tax_amount: lot.tax_amount,
        compensation_amount: lot.compensation_amount,
        hold_ppm: lot.hold_ppm,
        adjustment_ppm: lot.adjustment_ppm,
        leader_held_nanos: lot.leader_held_nanos,
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{StrictSettlementReport, decode_settlement_report};

    fn auction() -> Value {
        json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "world_id": "main",
            "sequence": 1,
            "status": "settled",
            "preview_scheduled_at": "2026-11-01T00:00:00Z",
            "scheduled_open_at": "2026-11-02T00:00:00Z",
            "preview_opened_at": "2026-11-01T00:00:00Z",
            "preview_manual_open": false,
            "preview_revision": 0,
            "opened_at": "2026-11-02T00:00:00Z",
            "final_phase_at": "2026-11-03T00:00:00Z",
            "settled_at": "2026-11-03T01:00:00Z",
            "cancelled_at": null,
            "cancelled_reason": null,
            "cancelled_by": null,
            "state_version": 5,
            "created_at": "2026-10-31T00:00:00Z",
            "updated_at": "2026-11-03T01:00:00Z"
        })
    }

    fn lot() -> Value {
        json!({
            "id": "00000000-0000-4000-8000-000000000002",
            "lot_number": 1,
            "status": "unsold",
            "equipment_name": "Legendary Ethereal Rapier",
            "equipment_category": "One-handed Weapon",
            "game_equipment_url": "https://hentaiverse.org/equip/123",
            "seller": null,
            "winner": null,
            "final_price": 0,
            "listed_price": 100,
            "buyer_amount": 0,
            "seller_amount": 0,
            "tax_amount": 0,
            "compensation_amount": 0,
            "hold_ppm": 0,
            "adjustment_ppm": 0,
            "leader_held_nanos": 0,
            "adjustment": {}
        })
    }

    fn report(lots: Vec<Value>) -> Value {
        json!({ "auction": auction(), "lots": lots, "settlement": null })
    }

    fn without(mut value: Value, key: &str) -> Value {
        value.as_object_mut().expect("测试载荷必须是对象").remove(key);
        value
    }

    #[test]
    fn explicit_null_settlement_and_parties_decode_as_none() {
        let data: StrictSettlementReport =
            serde_json::from_value(report(vec![lot()])).expect("显式 null");
        let decoded = decode_settlement_report(data).expect("合法结算报告");
        assert!(decoded.settlement.is_none());
        assert_eq!(decoded.lots[0].seller, None);
        assert_eq!(decoded.lots[0].winner, None);
    }

    #[test]
    fn missing_nullable_settlement_fields_fail_closed() {
        // 缺键不能被当作“未结算/无买卖方”：必填可空字段只接受显式 null。
        let missing_settlement = without(report(vec![lot()]), "settlement");
        assert!(serde_json::from_value::<StrictSettlementReport>(missing_settlement).is_err());
        for key in ["seller", "winner"] {
            let missing_party = report(vec![without(lot(), key)]);
            assert!(
                serde_json::from_value::<StrictSettlementReport>(missing_party).is_err(),
                "缺少 {key} 必须拒绝"
            );
        }
    }
}
