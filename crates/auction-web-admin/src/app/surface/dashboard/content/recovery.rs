//! 结算恢复面板：逾期/待上架/失败动作计数与逐世界余额证据。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::derived::DashboardDerived;
use super::shared::{badge, card, text};

pub(super) fn recovery_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let recovery = snapshot.recovery.data.as_ref();
    let recovery_state = derived.recovery_state;
    let recovery_status = derived.recovery_status.clone();
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-recovery-title",
    h2 { id: "dashboard-recovery-title", {t!("dashboard-legacy-recovery-health")} }
    if let Some(recovery) = recovery {
        div { class: "admin-dashboard-cards",
            {card(
                t!("dashboard-legacy-recovery-health"),
                badge(recovery_state, recovery_status),
            )}
            {card(
                t!("dashboard-legacy-recovery-payment-overdue"),
                text(recovery.payment_overdue_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-recovery-relist-pending"),
                text(recovery.relist_pending_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-recovery-failed-actions"),
                text(recovery.failed_action_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-recovery-manual-reconcile"),
                text(recovery.manual_reconcile_count.to_string()),
            )}
            {card(
                t!("dashboard-legacy-recovery-oldest-overdue"),
                text(t!(
                    "dashboard-legacy-recovery-age",
                    seconds: recovery.oldest_overdue_age_seconds
                )),
            )}
        }
        h3 { {t!("dashboard-legacy-recovery-worlds")} }
        div { class: "admin-dashboard-cards",
            for world in recovery.worlds.iter() {
                {
                    let balance_known = world.balance_observed_at.is_some()
                        && !matches!(world.balance_status.as_str(), "unknown" | "unavailable");
                    let balance_tone = match world.balance_status.as_str() {
                        "ok" => "ok",
                        "low" | "stale" | "busy" => "warn",
                        _ => "neutral",
                    };
                    let evidence_tone = match world.evidence_status.as_str() {
                        "fresh" => "ok",
                        "stale" => "warn",
                        _ => "neutral",
                    };
                    let world_name = world.world.clone();
                    rsx! {
                        div { class: "admin-dashboard-card", key: "{world_name}",
                            div { class: "admin-dashboard-value",
                                span { class: "admin-dashboard-recovery-world",
                                    // 只记账状态保留后端历史观察，但不能把旧金额呈为当前余额。
                                    if world.balance_status != "not_applicable" {
                                        span {
                                            if balance_known {
                                                {t!(
                                                    "dashboard-legacy-recovery-balance-credits",
                                                    credits: world.balance_credits
                                                )}
                                            } else {
                                                {t!("dashboard-legacy-no-data")}
                                            }
                                        }
                                    }
                                    {badge(
                                        balance_tone,
                                        recovery_balance_status(&world.balance_status),
                                    )}
                                    span { class: "admin-dashboard-label",
                                        {t!("dashboard-legacy-recovery-evidence-label")}
                                        " "
                                        {badge(
                                            evidence_tone,
                                            recovery_evidence_status(&world.evidence_status),
                                        )}
                                    }
                                    if world.balance_status == "busy" {
                                        span { class: "admin-dashboard-label",
                                            {t!("dashboard-legacy-recovery-balance-busy-hint")}
                                        }
                                    }
                                }
                            }
                            div { class: "admin-dashboard-label", "{world_name}" }
                        }
                    }
                }
            }
        }
    } else {
        p { class: "admin-dashboard-status",
            {t!("dashboard-legacy-recovery-status-unavailable")}
        }
    }

    }
    }
}

fn recovery_balance_status(status: &str) -> String {
    match status {
        "not_applicable" => t!("dashboard-legacy-recovery-balance-not-applicable"),
        "ok" => t!("dashboard-legacy-recovery-balance-ok"),
        "low" => t!("dashboard-legacy-recovery-balance-low"),
        "stale" => t!("dashboard-legacy-recovery-balance-stale"),
        "busy" => t!("dashboard-legacy-recovery-balance-busy"),
        "unavailable" => t!("dashboard-legacy-recovery-balance-unavailable"),
        _ => t!("dashboard-legacy-recovery-balance-unknown"),
    }
}

fn recovery_evidence_status(status: &str) -> String {
    match status {
        "not_applicable" => t!("dashboard-legacy-recovery-evidence-not-applicable"),
        "fresh" => t!("dashboard-legacy-recovery-evidence-fresh"),
        "stale" => t!("dashboard-legacy-recovery-evidence-stale"),
        "unavailable" => t!("dashboard-legacy-recovery-evidence-unavailable"),
        _ => t!("dashboard-legacy-recovery-evidence-unknown"),
    }
}
