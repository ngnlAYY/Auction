//! Admin 结算与 recovery 的标签、金额和分页辅助。

use auction_web_shared::legacy_t as t;

use auction_web_shared::format_credit_amount as format_amount;

use crate::api::{AdminRecoveryAction, AdminSettlementLot};

pub(super) fn recovery_stage_label(stage: &str) -> String {
    match stage {
        "awaiting_payment" => t!("admin-recovery-stage-awaiting-payment"),
        "payment_overdue" => t!("admin-recovery-stage-payment-overdue"),
        "delivering_equipment" => t!("admin-recovery-stage-delivering"),
        "reclaiming" => t!("admin-recovery-stage-reclaiming"),
        "relist_pending" => t!("admin-recovery-stage-relist-pending"),
        "refunding" => t!("admin-recovery-stage-refunding"),
        "completed" => t!("admin-recovery-stage-completed"),
        _ => stage.to_owned(),
    }
}

pub(super) fn recovery_action_summary(actions: &[AdminRecoveryAction]) -> String {
    if actions.is_empty() {
        return t!("value-empty");
    }
    actions
        .iter()
        .map(|action| {
            format!(
                "{}: {}",
                recovery_action_kind_label(&action.kind),
                recovery_action_state_label(&action.state)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(super) fn price_adjustment_label(lot: &AdminSettlementLot) -> String {
    match lot.adjustment_ppm.cmp(&0) {
        std::cmp::Ordering::Greater => t!(
            "admin-settlement-penalty-value",
            amount: format_amount(lot.tax_amount)
        ),
        std::cmp::Ordering::Less => {
            let Some(discount) = lot.final_price.checked_sub(lot.listed_price) else {
                return t!("value-empty");
            };
            t!("admin-settlement-discount-value", amount: format_amount(discount))
        }
        std::cmp::Ordering::Equal => t!("value-empty"),
    }
}

pub(super) fn recovery_action_kind_label(kind: &str) -> String {
    match kind {
        "payment_reminder" => t!("admin-recovery-action-payment-reminder"),
        "reclaim_equipment_take" => t!("admin-recovery-action-reclaim"),
        "cancel_carrier_take" => t!("admin-recovery-action-cancel-carrier"),
        "recover_equipment_take" => t!("admin-recovery-action-recover-equipment"),
        "buyer_blacklist" => t!("admin-recovery-action-buyer-blacklist"),
        _ => kind.to_owned(),
    }
}

pub(super) fn recovery_action_state_label(state: &str) -> String {
    match state {
        "pending" => t!("admin-recovery-state-pending"),
        "prepared" => t!("admin-recovery-state-prepared"),
        "enqueued" => t!("admin-recovery-state-enqueued"),
        "satisfied" => t!("admin-recovery-state-satisfied"),
        "failed" => t!("admin-recovery-state-failed"),
        "manual_reconcile" => t!("admin-recovery-state-manual"),
        _ => state.to_owned(),
    }
}

pub(super) fn recovery_command_label(action: &str) -> String {
    match action {
        "reclaimed" => t!("admin-recovery-command-reclaimed"),
        "cancelled" => t!("admin-recovery-command-cancelled"),
        "equipment_recovered" => t!("admin-recovery-command-equipment-recovered"),
        "returned_after_three_cod_returns" => {
            t!("admin-recovery-command-returned-after-three-cod-returns")
        }
        _ => action.to_owned(),
    }
}

/// 只对完整权威报告切片；先做有界整数转换，不按用户数量分配内存。
pub(super) fn page_count(length: usize, size: i64) -> i64 {
    let length = i64::try_from(length).unwrap_or(i64::MAX);
    if length == 0 { 1 } else { 1 + (length - 1) / size.max(1) }
}

pub(super) fn page_slice<T: Clone>(items: &[T], page: i64, size: i64) -> Vec<T> {
    let offset = page.max(1).saturating_sub(1).saturating_mul(size.max(1));
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let size = usize::try_from(size.max(1)).unwrap_or(usize::MAX);
    items.iter().skip(offset).take(size).cloned().collect()
}
