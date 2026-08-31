//! Admin 结算与 recovery 页面装配。
//!
//! 结算报告、recovery 交互和展示辅助分别位于子模块；页面仍复刻旧版的操作顺序：先选择
//! 已结算拍卖，再读取结算 lot、资金池摘要和 recovery 安全投影。

use std::collections::HashMap;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{fetch_admin_auctions, fetch_admin_recovery, fetch_admin_settlement};

use super::{AdminActionFeedback, admin_api_error_message};

#[path = "settlement/helpers.rs"]
mod helpers;
#[path = "settlement/picker.rs"]
mod picker;
#[path = "settlement/recovery.rs"]
mod recovery;
#[path = "settlement/report.rs"]
mod report;

use picker::{AdminSettlementAuctionPager, AdminSettlementPicker};
use recovery::AdminRecoveryDialog;
use report::AdminSettlementReportView;

const PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, PartialEq)]
struct RecoveryConfirmationDraft {
    recovery_id: String,
    state_version: i64,
    action: String,
    equipment_name: String,
    lot_code: String,
}

/// 加载并展示旧版结算页面对应的完整 Rust 页面。
pub(super) fn page() -> Element {
    let selected_auction_id = use_signal(String::new);
    let auction_page = use_signal(|| 1_i64);
    let lot_page = use_signal(|| 1_i64);
    let recovery_page = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let recovery_reload_generation = use_signal(|| 0_u64);
    let confirmation = use_signal(|| None::<RecoveryConfirmationDraft>);
    let reason = use_signal(String::new);
    let action_busy = use_signal(|| false);
    let action_feedback = use_signal(|| None::<AdminActionFeedback>);
    // 旧页面按 recovery/version/action/reason 保存幂等 ID；409 关闭对话框后仍可复用。
    let recovery_command_ids = use_signal(HashMap::<String, Uuid>::new);

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
        let _ = recovery_reload_generation();
        let auction_id = selected_auction_id();
        async move {
            if auction_id.is_empty() {
                Ok(None)
            } else {
                fetch_admin_recovery(auction_id).await.map(Some)
            }
        }
    });

    let auctions_state = auctions.read_unchecked().clone();
    let settlement_state = settlement.read_unchecked().clone();
    let recovery_state = recovery.read_unchecked().clone();
    let selected = selected_auction_id();
    let auction_page_data = auctions_state.as_ref().and_then(|state| state.as_ref().ok()).cloned();
    let report = settlement_state
        .as_ref()
        .and_then(|state| state.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    let recovery_report = recovery_state
        .as_ref()
        .and_then(|state| state.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    let loading = auctions_state.is_none()
        || !selected.is_empty() && (settlement_state.is_none() || recovery_state.is_none());
    let load_error = match auctions_state.as_ref() {
        Some(Err(error)) => Some(admin_api_error_message(error)),
        _ if !selected.is_empty() => match settlement_state.as_ref() {
            Some(Err(error)) => Some(admin_api_error_message(error)),
            _ => match recovery_state.as_ref() {
                Some(Err(error)) => Some(admin_api_error_message(error)),
                _ => None,
            },
        },
        _ => None,
    };

    rsx! {
        section { class: "admin-auction-page", aria_labelledby: "settlement-title",
            h1 { id: "settlement-title", {t!("page-settlement-title")} }
            AdminSettlementPicker {
                page: auction_page_data.clone(),
                selected_auction_id,
                lot_page,
                recovery_page,
                confirmation,
                reason,
                action_feedback,
            }
            if loading {
                p { class: "admin-auction-status", role: "status", {t!("admin-settlement-loading")} }
            }
            if let Some(error) = load_error {
                p { class: "admin-auction-error", role: "alert", {error} }
            }
            if let Some(page) = auction_page_data {
                AdminSettlementAuctionPager {
                    page,
                    selected_auction_id,
                    auction_page,
                    lot_page,
                    recovery_page,
                }
            }
            if selected.is_empty() {
                p { class: "admin-auction-intro", {t!("admin-settlement-pick-prompt")} }
            }
            if let Some(report) = report {
                AdminSettlementReportView {
                    report,
                    recovery: recovery_report,
                    lot_page,
                    recovery_page,
                    action_busy,
                    confirmation,
                    reason,
                    action_feedback,
                }
            }
            if let Some(feedback) = action_feedback() {
                p {
                    class: if feedback.is_success() {
                        "admin-shell__inline-ok"
                    } else {
                        "admin-auction-error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("admin-recovery-action-success"))}
                }
            }
            if let Some(draft) = confirmation() {
                AdminRecoveryDialog {
                    draft,
                    confirmation,
                    reason,
                    busy: action_busy,
                    feedback: action_feedback,
                    command_ids: recovery_command_ids,
                    reload_generation,
                    recovery_reload_generation,
                }
            }
        }
    }
}
