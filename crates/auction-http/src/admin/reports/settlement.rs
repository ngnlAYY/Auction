//! Admin 结算报告的结果校验与契约映射。
//! 校验沿用旧版 Admin 快照的时间、生命周期与金额守恒不变量，失败时由 handler 失败关闭，
//! 错误状态不进入 wire。

use std::collections::BTreeSet;

use auction_application::settlement::{
    SettlementAuctionSnapshot, SettlementLotReport, SettlementReport,
};
use auction_contracts::{
    AdminSettlementAdjustmentResponse, AdminSettlementAuctionResponse, AdminSettlementLotResponse,
    AdminSettlementReportResponse, AdminSettlementSummaryResponse,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::mapping::timestamp;

pub(super) fn map_settlement_report(report: SettlementReport) -> AdminSettlementReportResponse {
    let equipment_prefix = match report.auction.world_id.as_str() {
        "isekai" => "https://hentaiverse.org/isekai/equip/",
        _ => "https://hentaiverse.org/equip/",
    };
    let auction = report.auction;
    AdminSettlementReportResponse {
        auction: AdminSettlementAuctionResponse {
            id: auction.id.to_string(),
            world_id: auction.world_id,
            sequence: auction.sequence,
            status: auction.status,
            preview_scheduled_at: timestamp(auction.preview_scheduled_at),
            scheduled_open_at: timestamp(auction.scheduled_open_at),
            preview_opened_at: auction.preview_opened_at.map(timestamp),
            preview_manual_open: auction.preview_manual_open,
            preview_revision: auction.preview_revision,
            opened_at: auction.opened_at.map(timestamp),
            final_phase_at: auction.final_phase_at.map(timestamp),
            settled_at: auction.settled_at.map(timestamp),
            cancelled_at: auction.cancelled_at.map(timestamp),
            cancelled_reason: auction.cancelled_reason,
            cancelled_by: auction.cancelled_by,
            state_version: auction.state_version,
            created_at: timestamp(auction.created_at),
            updated_at: timestamp(auction.updated_at),
        },
        settlement: report.settlement.map(|settlement| AdminSettlementSummaryResponse {
            opening_balance: settlement.opening_balance,
            tax_income: settlement.tax_income,
            discount_demand: settlement.discount_demand,
            compensation_out: settlement.compensation_out,
            closing_balance: settlement.closing_balance,
            movement_count: settlement.movement_count,
            settled_at: timestamp(settlement.settled_at),
        }),
        lots: report
            .lots
            .into_iter()
            .map(|lot| AdminSettlementLotResponse {
                id: lot.id.to_string(),
                lot_number: lot.lot_number,
                status: lot.status,
                equipment_name: lot.equipment_name,
                equipment_category: lot.equipment_category,
                game_equipment_url: format!("{equipment_prefix}{}", lot.external_equipment_id),
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
                adjustment: AdminSettlementAdjustmentResponse { policy: lot.adjustment.policy },
            })
            .collect(),
    }
}

pub(super) fn valid_settlement_report(report: &SettlementReport, auction_id: Uuid) -> bool {
    let auction = &report.auction;
    if auction.id.as_uuid() != auction_id
        || auction.sequence <= 0
        || auction.state_version <= 0
        || !valid_report_text(&auction.world_id, 64)
        || !matches!(
            auction.status.as_str(),
            "draft" | "preview" | "live" | "final" | "settling" | "settled" | "cancelled"
        )
        || !valid_report_time(auction.preview_scheduled_at)
        || auction.scheduled_open_at < auction.preview_scheduled_at
        || auction.preview_revision < 0
        || !valid_report_time(auction.created_at)
        || !valid_report_time(auction.updated_at)
        || auction.updated_at < auction.created_at
        || !valid_report_optional_time(auction.preview_opened_at)
        || !valid_report_optional_time(auction.opened_at)
        || !valid_report_optional_time(auction.final_phase_at)
        || !valid_report_optional_time(auction.settled_at)
        || !valid_report_optional_time(auction.cancelled_at)
        || !valid_report_auction_lifecycle(auction)
    {
        return false;
    }
    match &report.settlement {
        None => auction.status != "settled" && report.lots.is_empty(),
        Some(settlement) => {
            if auction.status != "settled"
                || auction.settled_at != Some(settlement.settled_at)
                || !valid_report_time(settlement.settled_at)
                || settlement.opening_balance < 0
                || settlement.tax_income < 0
                || settlement.discount_demand < 0
                || settlement.compensation_out < 0
                || settlement.compensation_out > settlement.discount_demand
                || settlement.closing_balance < 0
                || settlement.movement_count < 2
                || report.lots.is_empty()
            {
                return false;
            }
            let mut lot_ids = BTreeSet::new();
            let mut lot_codes = BTreeSet::new();
            let mut tax_income = 0_i64;
            let mut discount_demand = 0_i64;
            let mut compensation_out = 0_i64;
            let mut movement_count = 2_i64;
            for lot in &report.lots {
                if !lot_ids.insert(lot.id)
                    || !lot_codes.insert((lot.equipment_category.as_str(), lot.lot_number))
                {
                    return false;
                }
                if !valid_settlement_lot(lot) {
                    return false;
                }
                discount_demand = match lot
                    .final_price
                    .checked_sub(lot.listed_price)
                    .and_then(|discount| discount_demand.checked_add(discount))
                {
                    Some(value) => value,
                    None => return false,
                };
                tax_income = match tax_income.checked_add(lot.tax_amount) {
                    Some(value) => value,
                    None => return false,
                };
                compensation_out = match compensation_out.checked_add(lot.compensation_amount) {
                    Some(value) => value,
                    None => return false,
                };
                if lot.tax_amount > 0 {
                    let Some(next) = movement_count.checked_add(1) else { return false };
                    movement_count = next;
                }
                if lot.compensation_amount > 0 {
                    let Some(next) = movement_count.checked_add(1) else { return false };
                    movement_count = next;
                }
            }
            let Some(closing_balance) = settlement
                .opening_balance
                .checked_add(tax_income)
                .and_then(|value| value.checked_sub(compensation_out))
            else {
                return false;
            };
            discount_demand == settlement.discount_demand
                && tax_income == settlement.tax_income
                && compensation_out == settlement.compensation_out
                && closing_balance == settlement.closing_balance
                && movement_count == settlement.movement_count
        }
    }
}

/// 结算报告必须复用旧版 Admin 快照的时间与生命周期不变量，避免错误状态进入 wire。
fn valid_report_auction_lifecycle(auction: &SettlementAuctionSnapshot) -> bool {
    let preview_opened = auction.preview_opened_at;
    let opened = auction.opened_at;
    let final_phase = auction.final_phase_at;
    let settled = auction.settled_at;
    if auction.preview_manual_open && preview_opened.is_none()
        || opened.is_some_and(|opened| preview_opened.is_none_or(|preview| opened < preview))
        || final_phase.is_some_and(|final_phase| opened.is_none_or(|opened| final_phase < opened))
        || settled.is_some_and(|settled| opened.is_none_or(|opened| settled < opened))
    {
        return false;
    }
    // 与 Admin 页面解码的闭集一致：状态决定各阶段时刻是否必须存在，防止不一致快照进入 wire。
    let lifecycle_matches = match auction.status.as_str() {
        "draft" => {
            preview_opened.is_none()
                && opened.is_none()
                && final_phase.is_none()
                && settled.is_none()
        }
        "preview" => opened.is_none() && final_phase.is_none() && settled.is_none(),
        "live" => opened.is_some() && final_phase.is_none() && settled.is_none(),
        "final" | "settling" => opened.is_some() && final_phase.is_some() && settled.is_none(),
        "settled" => opened.is_some() && final_phase.is_some() && settled.is_some(),
        "cancelled" => opened.is_none() && final_phase.is_none() && settled.is_none(),
        _ => false,
    };
    if !lifecycle_matches {
        return false;
    }
    let cancelled = auction.status == "cancelled";
    let has_any_cancel_metadata = auction.cancelled_at.is_some()
        || auction.cancelled_reason.is_some()
        || auction.cancelled_by.is_some();
    let has_cancel_metadata = auction.cancelled_at.is_some()
        && auction.cancelled_reason.is_some()
        && auction.cancelled_by.is_some();
    if cancelled {
        has_cancel_metadata
            && auction.cancelled_reason.as_deref().is_some_and(valid_report_cancellation_reason)
            && auction.cancelled_by.as_deref().is_some_and(|actor| valid_report_text(actor, 256))
    } else {
        !has_any_cancel_metadata
    }
}

/// 结算 lot 的身份与状态边界保持原闭集，金额矩阵交领域按冻结模式验证。
fn valid_settlement_lot(lot: &SettlementLotReport) -> bool {
    if lot.id.is_nil()
        || lot.lot_number <= 0
        || !valid_report_text(&lot.equipment_name, 512)
        || !valid_report_text(&lot.equipment_category, 128)
        || lot.external_equipment_id.is_empty()
        || lot.external_equipment_id.len() > 32
        || !lot.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_report_optional_text(lot.seller.as_deref(), 512)
        || !valid_report_optional_text(lot.winner.as_deref(), 512)
        || lot.final_price < 0
        || lot.listed_price < 0
        || lot.buyer_amount < 0
        || lot.seller_amount < 0
        || lot.tax_amount < 0
        || lot.compensation_amount < 0
        || !(0..=1_000_000).contains(&lot.hold_ppm)
        || !(-1_000_000..=1_000_000).contains(&lot.adjustment_ppm)
        || lot.leader_held_nanos < 0
        || !valid_report_optional_text(lot.adjustment.policy.as_deref(), 64)
    {
        return false;
    }
    match lot.status.as_str() {
        "sold" => {
            let Some(winner) = lot.winner.as_deref() else {
                return false;
            };
            if !valid_report_text(winner, 512) || lot.final_price <= 0 || lot.listed_price <= 0 {
                return false;
            }
            let Some(buyer_side) = lot.buyer_amount.checked_add(lot.compensation_amount) else {
                return false;
            };
            let Some(seller_side) = lot.seller_amount.checked_add(lot.tax_amount) else {
                return false;
            };
            if buyer_side != seller_side || lot.tax_amount > 0 && lot.compensation_amount > 0 {
                return false;
            }
            let mode = match lot.adjustment.policy.as_deref() {
                Some("legacy_zero_v0") => auction_domain::SettlementMode::LegacyZeroV0,
                Some("hold_curve_v1") | None => auction_domain::SettlementMode::HoldCurveV1,
                Some("hold_curve_v2") => auction_domain::SettlementMode::HoldCurveV2,
                _ => return false,
            };
            settlement_report_line(lot).is_some_and(|line| line.validate_for_mode(mode).is_ok())
        }
        "unsold" => {
            lot.winner.is_none()
                && lot.final_price == 0
                && lot.listed_price == 0
                && lot.buyer_amount == 0
                && lot.seller_amount == 0
                && lot.tax_amount == 0
                && lot.compensation_amount == 0
                && lot.hold_ppm == 0
                && lot.adjustment_ppm == 0
        }
        _ => false,
    }
}

/// 只在 HTTP 边界转换报告字段，金额规则由领域按冻结模式统一校验。
fn settlement_report_line(lot: &SettlementLotReport) -> Option<auction_domain::SettlementLine> {
    let amount = |value| auction_domain::SettlementAmount::new(value).ok();
    Some(auction_domain::SettlementLine {
        lot_id: lot.id,
        end_price: amount(lot.final_price)?,
        listed_price: amount(lot.listed_price)?,
        buyer_amount: amount(lot.buyer_amount)?,
        seller_amount: amount(lot.seller_amount)?,
        tax_amount: amount(lot.tax_amount)?,
        compensation_amount: amount(lot.compensation_amount)?,
        hold_ppm: i64::from(lot.hold_ppm),
        adjustment_ppm: i64::from(lot.adjustment_ppm),
    })
}

fn valid_report_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value == value.trim() && value.len() <= maximum
}

fn valid_report_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| valid_report_text(value, maximum))
}

fn valid_report_time(value: Timestamp) -> bool {
    value.timestamp_nanos_opt().is_some()
}

fn valid_report_optional_time(value: Option<Timestamp>) -> bool {
    value.is_none_or(valid_report_time)
}

fn valid_report_cancellation_reason(value: &str) -> bool {
    valid_report_text(value, 1024) && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::core::ApplicationFuture;
    use auction_application::settlement::{
        SettlementAdjustment, SettlementAuctionSnapshot, SettlementLotReport, SettlementReport,
        SettlementReportReader, SettlementSummary,
    };
    use auction_domain::{AuctionId, LotId};
    use auction_time::Duration;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use crate::server::contract_fixtures::{
        AUCTION_ID, AdminFixturePorts, admin_get, assert_http, time,
    };

    struct FixtureSettlementReport(SettlementReport);

    impl SettlementReportReader for FixtureSettlementReport {
        fn settlement_report(
            &self,
            auction_id: AuctionId,
        ) -> ApplicationFuture<'_, Option<SettlementReport>> {
            Box::pin(async move { Ok((auction_id == self.0.auction.id).then(|| self.0.clone())) })
        }
    }

    #[tokio::test]
    async fn settlement_report_contract_fixtures() {
        let opened = time();
        let settled = opened + Duration::days(1);
        let sold = SettlementLotReport {
            id: LotId::from_uuid(Uuid::from_u128(0x4000_8000_0000_0000_0023)),
            lot_number: 1,
            status: "sold".to_owned(),
            equipment_name: "Legendary Staff".to_owned(),
            equipment_category: "Staff".to_owned(),
            external_equipment_id: "123".to_owned(),
            seller: Some("ExampleSeller".to_owned()),
            winner: Some("ExampleBuyer".to_owned()),
            final_price: 2_000,
            listed_price: 2_000,
            buyer_amount: 2_000,
            seller_amount: 2_000,
            tax_amount: 0,
            compensation_amount: 0,
            hold_ppm: 0,
            adjustment_ppm: 0,
            leader_held_nanos: 3_600_000_000_000,
            adjustment: SettlementAdjustment { policy: Some("legacy_zero_v0".to_owned()) },
        };
        let mut report = SettlementReport {
            auction: SettlementAuctionSnapshot {
                id: AuctionId::from_uuid(AUCTION_ID),
                world_id: "main".to_owned(),
                sequence: 5,
                status: "settled".to_owned(),
                preview_scheduled_at: opened - Duration::days(1),
                scheduled_open_at: opened,
                preview_opened_at: Some(opened - Duration::days(1)),
                preview_manual_open: false,
                preview_revision: 2,
                opened_at: Some(opened),
                final_phase_at: Some(settled - Duration::hours(1)),
                settled_at: Some(settled),
                cancelled_at: None,
                cancelled_reason: None,
                cancelled_by: None,
                state_version: 8,
                scheduled_end_at: Some(settled),
                effective_end_at: Some(settled),
                created_at: opened - Duration::days(2),
                updated_at: settled,
            },
            settlement: Some(SettlementSummary {
                opening_balance: 1000,
                tax_income: 0,
                discount_demand: 0,
                compensation_out: 0,
                closing_balance: 1000,
                movement_count: 2,
                settled_at: settled,
            }),
            lots: vec![sold.clone()],
        };
        let sold_report = report.clone();
        let mut long_hold_report = sold_report.clone();
        let long_settled = opened + Duration::days(106);
        long_hold_report.auction.final_phase_at = Some(long_settled - Duration::hours(1));
        long_hold_report.auction.settled_at = Some(long_settled);
        long_hold_report.auction.scheduled_end_at = Some(long_settled);
        long_hold_report.auction.effective_end_at = Some(long_settled);
        long_hold_report.auction.updated_at = long_settled;
        long_hold_report.settlement.as_mut().expect("结算摘要").settled_at = long_settled;
        long_hold_report.lots[0].leader_held_nanos = 9_072_000_000_000_000;
        report.lots = vec![SettlementLotReport {
            status: "unsold".to_owned(),
            winner: None,
            final_price: 0,
            listed_price: 0,
            buyer_amount: 0,
            seller_amount: 0,
            leader_held_nanos: 0,
            adjustment: SettlementAdjustment { policy: None },
            ..sold
        }];
        let url = format!("/api/v1/admin/auctions/{AUCTION_ID}/settlement-report");
        for (name, report) in [
            ("admin/settlement-report", sold_report),
            ("admin/settlement-report-unsold", report),
            ("admin/settlement-report-long-hold", long_hold_report),
        ] {
            let state = AdminFixturePorts {
                settlement_report: Some(Arc::new(FixtureSettlementReport(report))),
                ..Default::default()
            }
            .state();
            assert_http(name, StatusCode::OK, admin_get(state, &url).await).await;
        }
    }
}
