//! Admin 结算与恢复契约映射及结果校验。

use super::super::*;
use super::*;

pub(crate) fn map_settlement_report(report: SettlementReport) -> AdminSettlementReportResponse {
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
            scheduled_end_at: auction.scheduled_end_at.map(timestamp),
            effective_end_at: auction.effective_end_at.map(timestamp),
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
                seller: lot.seller,
                winner: lot.winner,
                final_price: lot.final_price,
                buyer_amount: lot.buyer_amount,
                seller_amount: lot.seller_amount,
                tax_amount: lot.tax_amount,
                compensation_amount: lot.compensation_amount,
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
        || auction.world_id.is_empty()
        || auction.status.is_empty()
        || auction.preview_revision < 0
        || auction.created_at > auction.updated_at
    {
        return false;
    }
    match &report.settlement {
        None => auction.status != "settled" && report.lots.is_empty(),
        Some(settlement) => {
            if auction.status != "settled"
                || auction.settled_at != Some(settlement.settled_at)
                || settlement.opening_balance < 0
                || settlement.tax_income < 0
                || settlement.discount_demand < 0
                || settlement.compensation_out < 0
                || settlement.closing_balance < 0
                || settlement.movement_count < 2
                || report.lots.is_empty()
            {
                return false;
            }
            report.lots.iter().all(|lot| {
                !lot.id.is_nil()
                    && lot.lot_number > 0
                    && matches!(lot.status.as_str(), "sold" | "unsold")
                    && !lot.equipment_name.is_empty()
                    && lot.final_price >= 0
                    && lot.buyer_amount >= 0
                    && lot.seller_amount >= 0
                    && lot.tax_amount >= 0
                    && lot.compensation_amount >= 0
                    && lot.adjustment.policy.as_ref().is_none_or(|policy| {
                        !policy.is_empty() && policy.len() <= 64 && policy == policy.trim()
                    })
            })
        }
    }
}

pub(crate) fn map_recovery_report(report: RecoveryReport) -> AdminRecoveryReportResponse {
    AdminRecoveryReportResponse {
        auction_id: report.auction_id.to_string(),
        recoveries: report
            .recoveries
            .into_iter()
            .map(|recovery| AdminRecoveryResponse {
                id: recovery.id.to_string(),
                auction_id: recovery.auction_id.to_string(),
                lot_id: recovery.lot_id.to_string(),
                lot_number: recovery.lot_number,
                world_id: recovery.world_id,
                equipment_name: recovery.equipment_name,
                stage: recovery.stage,
                resolution: recovery.resolution,
                state_version: recovery.state_version,
                payment_deadline_at: timestamp(recovery.payment_deadline_at),
                reminder_count: recovery.reminder_count,
                reminder_limit: recovery.reminder_limit,
                custody_proof_kind: recovery.custody_proof_kind,
                custody_proof_observed_at: recovery.custody_proof_observed_at.map(timestamp),
                blocking_code: recovery.blocking_code,
                manual_reconcile_delivery_id: recovery
                    .manual_reconcile_delivery_id
                    .map(|id| id.to_string()),
                relisted_auction_id: recovery.relisted_auction_id.map(|id| id.to_string()),
                relisted_lot_id: recovery.relisted_lot_id.map(|id| id.to_string()),
                actions: recovery
                    .actions
                    .into_iter()
                    .map(|action| AdminRecoveryActionResponse {
                        id: action.id.to_string(),
                        kind: action.kind,
                        state: action.state,
                        delivery_id: action.delivery_id.map(|id| id.to_string()),
                        failure_code: action.failure_code,
                        proof_kind: action.proof_kind,
                        proof_observed_at: action.proof_observed_at.map(timestamp),
                        updated_at: timestamp(action.updated_at),
                    })
                    .collect(),
                allowed_commands: recovery.allowed_commands,
                updated_at: timestamp(recovery.updated_at),
            })
            .collect(),
    }
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
                !matches!(resolution.as_str(), "reclaimed" | "cancelled" | "equipment_recovered")
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
