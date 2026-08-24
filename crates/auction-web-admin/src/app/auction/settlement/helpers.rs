//! Admin 结算与 recovery 的标签、金额和分页辅助。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{AdminAuction, AdminRecoveryAction};

pub(super) fn stable_command_id(
    mut command_state: Signal<Option<(String, Uuid)>>,
    signature: String,
) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

pub(super) fn auction_option_label(auction: &AdminAuction) -> String {
    format!(
        "{} #{} ({})",
        auction_world_label(&auction.world_id),
        auction.sequence,
        auction_status_label(&auction.status)
    )
}

pub(super) fn auction_world_label(world_id: &str) -> String {
    match world_id {
        "main" => t!("admin-auctions-world-main"),
        "isekai" => t!("admin-auctions-world-isekai"),
        _ => world_id.to_owned(),
    }
}

fn auction_status_label(status: &str) -> String {
    match status {
        "settled" => t!("admin-auctions-status-settled"),
        "settling" => t!("admin-auctions-status-settling"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
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

pub(super) fn recovery_stage_class(stage: &str) -> &'static str {
    match stage {
        "completed" => "admin-settlement__stage admin-settlement__stage--completed",
        "payment_overdue" | "reclaiming" | "refunding" => {
            "admin-settlement__stage admin-settlement__stage--attention"
        }
        _ => "admin-settlement__stage admin-settlement__stage--active",
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

pub(super) fn hold_share(tax_amount: i64, final_price: i64) -> String {
    if final_price <= 0 {
        return t!("value-empty");
    }
    let scaled_percent =
        tax_amount.max(0).saturating_mul(10_000).checked_div(final_price).unwrap_or_default();
    let whole = scaled_percent / 100;
    let fraction = scaled_percent % 100;
    if fraction == 0 {
        format!("{whole}%")
    } else if fraction % 10 == 0 {
        format!("{whole}.{}%", fraction / 10)
    } else {
        format!("{whole}.{fraction:02}%")
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
