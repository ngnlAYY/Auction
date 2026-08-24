//! Admin 结算与 recovery 页面装配。
//!
//! 结算报告、recovery 交互和展示辅助分别位于子模块；页面仍复刻旧版的操作顺序：先选择
//! 已结算拍卖，再读取结算 lot、资金池摘要和 recovery 安全投影。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{fetch_admin_auctions, fetch_admin_recovery, fetch_admin_settlement};

use super::{AdminLoadError, AdminPanel};

#[path = "settlement/helpers.rs"]
mod helpers;
#[path = "settlement/picker.rs"]
mod picker;
#[path = "settlement/recovery.rs"]
mod recovery;
#[path = "settlement/report.rs"]
mod report;

use picker::AdminSettlementPicker;
use recovery::AdminRecoveryDialog;
use report::AdminSettlementReportView;

const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, PartialEq)]
struct RecoveryConfirmationDraft {
    recovery_id: String,
    state_version: i64,
    action: String,
    equipment_name: String,
    lot_number: i32,
}

/// 加载并展示旧版结算页面对应的完整 Rust 页面。
pub(super) fn page() -> Element {
    let selected_auction_id = use_signal(String::new);
    let auction_page = use_signal(|| 1_i64);
    let lot_page = use_signal(|| 1_i64);
    let recovery_page = use_signal(|| 1_i64);
    let mut reload_generation = use_signal(|| 0_u64);
    let confirmation = use_signal(|| None::<RecoveryConfirmationDraft>);
    let reason = use_signal(String::new);
    let action_busy = use_signal(|| false);
    let action_feedback = use_signal(|| None::<bool>);

    let auctions = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_auctions("all".to_owned(), "settled".to_owned(), auction_page())
    });
    let settlement = use_resource(move || {
        let _ = reload_generation();
        let auction_id = selected_auction_id();
        async move {
            if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_settlement(auction_id).await.map(Some)
            }
        }
    });
    let recovery = use_resource(move || {
        let _ = reload_generation();
        let auction_id = selected_auction_id();
        async move {
            if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_recovery(auction_id).await.map(Some)
            }
        }
    });

    match &*auctions.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-settlement-title"), detail: t!("admin-settlement-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-settlement-title"),
                detail: t!("admin-settlement-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => {
            let settlement_state = settlement.read_unchecked().clone();
            let recovery_state = recovery.read_unchecked().clone();
            let selected = selected_auction_id();
            rsx! {
                section { class: "admin-panel admin-settlement", aria_labelledby: "admin-settlement-title",
                    h1 { id: "admin-settlement-title", {t!("page-settlement-title")} }
                    p { class: "admin-panel__note", {t!("page-settlement-note")} }
                    AdminSettlementPicker {
                        page: page.clone(),
                        selected_auction_id,
                        auction_page,
                        lot_page,
                        recovery_page,
                        confirmation,
                        reason,
                        action_feedback,
                    }
                    if selected.is_empty() {
                        p { class: "admin-panel__note", {t!("admin-settlement-pick-prompt")} }
                    }
                    if !selected.is_empty() {
                        if settlement_state.is_none() {
                            p { class: "admin-panel__note", role: "status", {t!("admin-settlement-loading")} }
                        }
                        if let Some(Err(_)) = settlement_state.as_ref() {
                            div { class: "admin-panel__note", role: "alert",
                                p { {t!("admin-settlement-error")} }
                                button {
                                    r#type: "button",
                                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                                    {t!("admin-retry")}
                                }
                            }
                        }
                        if let Some(Ok(Some(report))) = settlement_state.clone() {
                            AdminSettlementReportView {
                                report,
                                recovery: recovery_state.clone(),
                                lot_page,
                                recovery_page,
                                confirmation,
                                reason,
                                action_feedback,
                            }
                        }
                        if recovery_state.is_none() {
                            p { class: "admin-panel__note", role: "status", {t!("admin-recovery-loading")} }
                        }
                        if let Some(Err(_)) = recovery_state.as_ref() {
                            div { class: "admin-panel__note", role: "alert",
                                p { {t!("admin-recovery-error")} }
                                button {
                                    r#type: "button",
                                    onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                                    {t!("admin-retry")}
                                }
                            }
                        }
                    }
                    if let Some(draft) = confirmation() {
                        AdminRecoveryDialog {
                            draft,
                            confirmation,
                            reason,
                            busy: action_busy,
                            feedback: action_feedback,
                            reload_generation,
                        }
                    }
                    if let Some(success) = action_feedback() {
                        p {
                            class: if success {
                                "admin-panel__note action-feedback action-feedback--success"
                            } else {
                                "admin-panel__note action-feedback action-feedback--error"
                            },
                            role: if success { "status" } else { "alert" },
                            {if success { t!("admin-recovery-action-success") } else { t!("admin-recovery-action-error") }}
                        }
                    }
                }
            }
        }
    }
}
