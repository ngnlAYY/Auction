//! Admin 结算与恢复契约映射及结果校验。

use crate::mapping::timestamp;
use auction_application::recovery::{
    RecoveryConfirmationResult, RecoveryReport, allowed_recovery_commands,
};
use auction_application::settlement::SettlementReport;
use auction_application::settlement::{SettlementAuctionSnapshot, SettlementLotReport};
use auction_contracts::{
    AdminRecoveryActionResponse, AdminRecoveryConfirmationResultResponse,
    AdminRecoveryReportResponse, AdminRecoveryResponse, AdminSettlementAdjustmentResponse,
    AdminSettlementAuctionResponse, AdminSettlementLotResponse, AdminSettlementReportResponse,
    AdminSettlementSummaryResponse,
};
use auction_domain::SettlementId;
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(crate) fn map_settlement_report(report: SettlementReport) -> AdminSettlementReportResponse {
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

pub(crate) fn valid_settlement_report(report: &SettlementReport, auction_id: Uuid) -> bool {
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
                    movement_count += 1;
                }
                if lot.compensation_amount > 0 {
                    movement_count += 1;
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
    if auction.preview_manual_open && auction.preview_opened_at.is_none()
        || auction
            .opened_at
            .is_some_and(|opened| auction.preview_opened_at.is_none_or(|preview| opened < preview))
        || auction
            .final_phase_at
            .is_some_and(|final_phase| auction.opened_at.is_none_or(|opened| final_phase < opened))
        || auction
            .settled_at
            .is_some_and(|settled| auction.opened_at.is_none_or(|opened| settled < opened))
    {
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

fn valid_report_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}

fn valid_report_optional_time(value: Option<DateTime<Utc>>) -> bool {
    value.is_none_or(valid_report_time)
}

fn valid_report_cancellation_reason(value: &str) -> bool {
    valid_report_text(value, 1024) && !value.chars().any(char::is_control)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryReportMappingError {
    UnknownErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicRecoveryErrorCode {
    InvalidFact,
    RecipientUnavailable,
    DeliveryConflict,
    DeliveryUnavailable,
    DeliveryFailed,
    StaleClaim,
    EvidenceNotObserved,
    EvidenceUnavailable,
    EvidenceMismatch,
    OutcomeUnknown,
    RelistUnavailable,
}

impl PublicRecoveryErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidFact => "invalid_recovery_fact",
            Self::RecipientUnavailable => "recovery_recipient_unavailable",
            Self::DeliveryConflict => "recovery_delivery_conflict",
            Self::DeliveryUnavailable => "recovery_delivery_unavailable",
            Self::DeliveryFailed => "recovery_delivery_failed",
            Self::StaleClaim => "stale_recovery_claim",
            Self::EvidenceNotObserved => "recovery_evidence_not_observed",
            Self::EvidenceUnavailable => "recovery_evidence_unavailable",
            Self::EvidenceMismatch => "recovery_evidence_mismatch",
            Self::OutcomeUnknown => "recovery_outcome_unknown",
            Self::RelistUnavailable => "recovery_relist_unavailable",
        }
    }
}

fn map_recovery_error_code(
    value: &str,
) -> Result<PublicRecoveryErrorCode, RecoveryReportMappingError> {
    match value {
        // 旧 v1 已持久化的公开码必须保持原值，内部实现码只在这个防腐边界归并。
        "invalid_recovery_fact" => Ok(PublicRecoveryErrorCode::InvalidFact),
        "recovery_recipient_unavailable" => Ok(PublicRecoveryErrorCode::RecipientUnavailable),
        "recovery_delivery_conflict" => Ok(PublicRecoveryErrorCode::DeliveryConflict),
        "recovery_delivery_unavailable" => Ok(PublicRecoveryErrorCode::DeliveryUnavailable),
        "recovery_delivery_failed" => Ok(PublicRecoveryErrorCode::DeliveryFailed),
        "stale_recovery_claim" => Ok(PublicRecoveryErrorCode::StaleClaim),
        "recovery_evidence_not_observed" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "recovery_evidence_unavailable" => Ok(PublicRecoveryErrorCode::EvidenceUnavailable),
        "recovery_evidence_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        "recovery_outcome_unknown" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "recovery_relist_unavailable" => Ok(PublicRecoveryErrorCode::RelistUnavailable),
        // 旧赛季装备已随 isekai 换季清空，属于永久不能重挂。
        "relist_season_closed" => Ok(PublicRecoveryErrorCode::RelistUnavailable),
        "evidence_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        // 成功页但远端引用不能证明的旧语义是 outcome_unknown，不能伪装成已观察到的证据缺失。
        "delivery_reference_mismatch"
        | "delivery_reference_missing"
        | "delivery_reference_unproven"
        | "delivery_outcome_unknown" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "delivery_failed" | "send_rejected" | "invalid_payload" => {
            Ok(PublicRecoveryErrorCode::DeliveryFailed)
        }
        "send_rejected_after_stage"
        | "write_unknown"
        | "gateway_outcome_unknown"
        | "invalid_gateway_result"
        | "lease_expired_outcome_unknown"
        | "task_panic"
        | "preflight_unproven" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "delivery_unavailable"
        | "recovery_observation_failed"
        | "recovery_buyer_blacklist_unavailable"
        | "recovery_buyer_blacklist_owner_unavailable"
        | "recovery_activation_unavailable"
        | "recovery_activation_link_failed"
        | "credential_unavailable"
        | "secret_not_found"
        | "invalid_secret"
        | "login_key_unavailable"
        | "session_expired"
        | "rate_limited"
        | "remote_temporary"
        | "response_too_large" => Ok(PublicRecoveryErrorCode::DeliveryUnavailable),
        "recovery_delivery_pending" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "recovery_recipient_missing" | "recovery_recipient_ineligible" | "recipient_not_found" => {
            Ok(PublicRecoveryErrorCode::RecipientUnavailable)
        }
        "recovery_buyer_blacklist_result_invalid"
        | "recovery_activation_result_invalid"
        | "recovery_delivery_result_invalid"
        | "recovery_delivery_link_failed" => Ok(PublicRecoveryErrorCode::DeliveryConflict),
        "recovery_request_hash_failed"
        | "recovery_prepare_identity_missing"
        | "recovery_activation_hash_failed"
        | "recovery_source_mail_missing"
        | "unsupported_recovery_action"
        | "configuration" => Ok(PublicRecoveryErrorCode::InvalidFact),
        "not_observed" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "protocol_drift" | "identity_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        _ => Err(RecoveryReportMappingError::UnknownErrorCode),
    }
}

fn map_optional_recovery_error_code(
    value: Option<String>,
) -> Result<Option<String>, RecoveryReportMappingError> {
    value
        .as_deref()
        .map(map_recovery_error_code)
        .transpose()
        .map(|value| value.map(PublicRecoveryErrorCode::as_str).map(ToOwned::to_owned))
}

pub(crate) fn map_recovery_report(
    report: RecoveryReport,
) -> Result<AdminRecoveryReportResponse, RecoveryReportMappingError> {
    let recoveries = report
        .recoveries
        .into_iter()
        .map(|recovery| -> Result<AdminRecoveryResponse, RecoveryReportMappingError> {
            let actions = recovery
                .actions
                .into_iter()
                .map(|action| -> Result<AdminRecoveryActionResponse, RecoveryReportMappingError> {
                    Ok(AdminRecoveryActionResponse {
                        id: action.id.to_string(),
                        kind: action.kind,
                        state: action.state,
                        delivery_id: action.delivery_id.map(|id| id.to_string()),
                        failure_code: map_optional_recovery_error_code(action.failure_code)?,
                        proof_kind: action.proof_kind,
                        proof_observed_at: action.proof_observed_at.map(timestamp),
                        updated_at: timestamp(action.updated_at),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AdminRecoveryResponse {
                id: recovery.id.to_string(),
                auction_id: recovery.auction_id.to_string(),
                lot_id: recovery.lot_id.to_string(),
                lot_number: recovery.lot_number,
                world_id: recovery.world_id,
                equipment_name: recovery.equipment_name,
                equipment_category: recovery.equipment_category,
                stage: recovery.stage,
                resolution: recovery.resolution,
                state_version: recovery.state_version,
                payment_deadline_at: timestamp(recovery.payment_deadline_at),
                reminder_count: recovery.reminder_count,
                reminder_limit: recovery.reminder_limit,
                custody_proof_kind: recovery.custody_proof_kind,
                custody_proof_observed_at: recovery.custody_proof_observed_at.map(timestamp),
                blocking_code: map_optional_recovery_error_code(recovery.blocking_code)?,
                manual_reconcile_delivery_id: recovery
                    .manual_reconcile_delivery_id
                    .map(|id| id.to_string()),
                relisted_auction_id: recovery.relisted_auction_id.map(|id| id.to_string()),
                relisted_lot_id: recovery.relisted_lot_id.map(|id| id.to_string()),
                actions,
                allowed_commands: recovery.allowed_commands,
                updated_at: timestamp(recovery.updated_at),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AdminRecoveryReportResponse { auction_id: report.auction_id.to_string(), recoveries })
}

pub(crate) fn map_recovery_confirmation(
    result: RecoveryConfirmationResult,
) -> AdminRecoveryConfirmationResultResponse {
    AdminRecoveryConfirmationResultResponse {
        recovery_id: result.recovery_id.to_string(),
        action: result.action.as_str().to_owned(),
        stage: result.stage,
        resolution: result.resolution,
        state_version: result.state_version,
        relisted_auction_id: result.relisted_auction_id.map(|id| id.to_string()),
        relisted_lot_id: result.relisted_lot_id.map(|id| id.to_string()),
    }
}

pub(crate) fn valid_recovery_report(report: &RecoveryReport, auction_id: Uuid) -> bool {
    if report.auction_id.as_uuid() != auction_id {
        return false;
    }
    let mut recovery_ids = std::collections::BTreeSet::new();
    let mut lot_ids = std::collections::BTreeSet::new();
    for recovery in &report.recoveries {
        if recovery.id.is_nil()
            || recovery.auction_id.as_uuid() != auction_id
            || recovery.lot_id.is_nil()
            || recovery.lot_number <= 0
            || !matches!(recovery.world_id.as_str(), "main" | "isekai")
            || recovery.equipment_name.is_empty()
            || recovery.equipment_category.is_empty()
            || !matches!(
                recovery.stage.as_str(),
                "awaiting_payment"
                    | "payment_overdue"
                    | "delivering_equipment"
                    | "reclaiming"
                    | "relist_pending"
                    | "refunding"
                    | "completed"
            )
            || recovery.resolution.as_ref().is_some_and(|resolution| {
                !matches!(
                    resolution.as_str(),
                    "reclaimed"
                        | "cancelled"
                        | "equipment_recovered"
                        | "returned_after_three_cod_returns"
                )
            })
            || recovery.state_version <= 0
            || recovery.payment_deadline_at.timestamp() <= 0
            || recovery.reminder_limit != 1
            || recovery.reminder_count < 0
            || recovery.reminder_count > recovery.reminder_limit
            || recovery.updated_at.timestamp() <= 0
            || recovery.blocking_code.as_ref().is_some_and(|code| code.is_empty())
            || recovery.manual_reconcile_delivery_id.is_some_and(SettlementId::is_nil)
            || recovery.relisted_auction_id.is_some_and(auction_domain::AuctionId::is_nil)
            || recovery.relisted_lot_id.is_some_and(auction_domain::LotId::is_nil)
            || recovery.relisted_auction_id.is_some() != recovery.relisted_lot_id.is_some()
            || !recovery_ids.insert(recovery.id.as_uuid())
            || !lot_ids.insert(recovery.lot_id.as_uuid())
        {
            return false;
        }
        let mut action_ids = std::collections::BTreeSet::new();
        for action in &recovery.actions {
            if action.id.is_nil()
                || !matches!(
                    action.kind.as_str(),
                    "payment_reminder"
                        | "reclaim_equipment_take"
                        | "cancel_carrier_take"
                        | "recover_equipment_take"
                        | "buyer_blacklist"
                )
                || !matches!(
                    action.state.as_str(),
                    "pending"
                        | "prepared"
                        | "enqueued"
                        | "satisfied"
                        | "failed"
                        | "manual_reconcile"
                )
                || action.delivery_id.is_some_and(SettlementId::is_nil)
                || action.failure_code.as_ref().is_some_and(|code| code.is_empty())
                || action.proof_kind.as_ref().is_some_and(String::is_empty)
                || action.updated_at.timestamp() <= 0
                || !action_ids.insert(action.id.as_uuid())
            {
                return false;
            }
        }
        if recovery.allowed_commands != allowed_recovery_commands(recovery)
            || recovery.allowed_commands.len() > 1
            || recovery.allowed_commands.iter().any(|command| {
                !matches!(command.as_str(), "reclaimed" | "cancelled" | "equipment_recovered")
            })
        {
            return false;
        }
    }
    true
}
