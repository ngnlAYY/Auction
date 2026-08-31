//! Admin 结算与 recovery 的标签、金额和分页辅助。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use auction_web_shared::format_credit_amount as format_amount;

use crate::api::{AdminAuction, AdminRecoveryAction, AdminSettlementLot};

pub(super) fn stable_command_id(
    mut command_ids: Signal<std::collections::HashMap<String, Uuid>>,
    signature: String,
) -> Uuid {
    if let Some(command_id) = command_ids.read().get(&signature).copied() {
        return command_id;
    }
    let command_id = Uuid::new_v4();
    command_ids.write().insert(signature, command_id);
    command_id
}

pub(super) fn auction_option_label(auction: &AdminAuction) -> String {
    format!("{} #{} ({})", auction.world_id, auction.sequence, auction.status)
}

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

pub(super) fn format_ppm(value: i32, show_positive_sign: bool) -> String {
    let absolute = i64::from(value).abs();
    let whole = absolute / 10_000;
    let fraction = absolute % 10_000;
    let number = if fraction == 0 {
        whole.to_string()
    } else {
        let fraction = format!("{fraction:04}").trim_end_matches('0').to_owned();
        format!("{whole}.{fraction}")
    };
    let sign = if value < 0 {
        "−"
    } else if value > 0 && show_positive_sign {
        "+"
    } else {
        ""
    };
    format!("{sign}{number}%")
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
        _ => action.to_owned(),
    }
}

pub(super) fn page_count(length: usize) -> usize {
    length.div_ceil(super::PAGE_SIZE).max(1)
}

pub(super) fn page_slice<T: Clone>(items: &[T], page: i64) -> Vec<T> {
    let page = page.max(1) as usize;
    items.iter().skip((page - 1) * super::PAGE_SIZE).take(super::PAGE_SIZE).cloned().collect()
}
