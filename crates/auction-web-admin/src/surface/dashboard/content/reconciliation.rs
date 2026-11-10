//! 资产对账面板：托管缺失与偿付缺口的逐世界状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::AdminDashboardSnapshot;

use super::derived::DashboardDerived;
use super::shared::{badge, card, text};

pub(super) fn reconciliation_panel(
    snapshot: &AdminDashboardSnapshot,
    derived: &DashboardDerived,
) -> Element {
    let reconciliation = snapshot.reconciliation.data.as_ref();
    let reconciliation_state = derived.reconciliation_state;
    let reconciliation_drift = derived.reconciliation_drift;
    rsx! {
    section { class: "admin-dashboard-panel", aria_labelledby: "dashboard-reconciliation-title",
    h2 { id: "dashboard-reconciliation-title", {t!("dashboard-reconciliation-title")} }
    if let Some(reconciliation) = reconciliation {
        div { class: "admin-dashboard-cards",
            {card(
                t!("dashboard-reconciliation-state"),
                badge(reconciliation_state, if reconciliation_drift {
                    t!("dashboard-reconciliation-differences")
                } else {
                    t!("dashboard-reconciliation-ok")
                }),
            )}
            {card(
                t!("dashboard-reconciliation-custody-age"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: reconciliation.custody_report_age_seconds
                )),
            )}
            {card(
                t!("dashboard-reconciliation-solvency-age"),
                text(t!(
                    "dashboard-legacy-duration-seconds",
                    seconds: reconciliation.solvency_report_age_seconds
                )),
            )}
        }
        h3 { {t!("dashboard-reconciliation-worlds")} }
        div { class: "admin-dashboard-cards",
            for world in reconciliation.worlds.iter() {
                {
                    let world_name = world.world.clone();
                    let missing_tone = if world.custody_missing_count > 0 { "error" } else { "ok" };
                    let shortfall_tone =
                        if world.solvency_shortfall_credits > 0 { "error" } else { "ok" };
                    rsx! {
                        div { class: "admin-dashboard-card", key: "{world_name}",
                            div { class: "admin-dashboard-value",
                                span {
                                    {t!("dashboard-reconciliation-custody")}
                                    " "
                                    {badge(missing_tone, world.custody_missing_count.to_string())}
                                }
                                span { class: "admin-dashboard-label",
                                    {t!("dashboard-reconciliation-solvency")}
                                    " "
                                    {badge(
                                        shortfall_tone,
                                        t!(
                                            "dashboard-reconciliation-credits",
                                            credits: world.solvency_shortfall_credits
                                        ),
                                    )}
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
            {t!("dashboard-reconciliation-status-unavailable")}
        }
    }

    }
    }
}
