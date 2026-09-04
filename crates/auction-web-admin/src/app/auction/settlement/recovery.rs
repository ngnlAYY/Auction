//! Admin recovery 表格、命令确认对话框和幂等提交交互。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminRecovery, AdminRecoveryReport, confirm_admin_recovery};
use crate::app::surface::AdminActionFeedback;

use super::RecoveryConfirmationDraft;
use super::helpers::{
    recovery_action_summary, recovery_command_label, recovery_stage_label, stable_command_id,
};
use super::report::AdminSettlementPager;

#[component]
pub(super) fn AdminRecoveryTable(
    report: Option<AdminRecoveryReport>,
    current_page: i64,
    total_pages: usize,
    mut page: Signal<i64>,
    action_busy: Signal<bool>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let rows = report
        .as_ref()
        .map(|report| super::helpers::page_slice(&report.recoveries, current_page))
        .unwrap_or_default();
    rsx! {
        section { class: "admin-auction-report admin-auction-recovery", aria_labelledby: "settlement-recovery-title",
            h2 { id: "settlement-recovery-title", {t!("admin-recovery-title")} }
            if let Some(report) = report {
                if report.recoveries.is_empty() {
                    p { class: "admin-auction-status", {t!("admin-recovery-empty")} }
                } else {
                    div { class: "admin-settlement__table-wrap",
                        table { class: "admin-shell__table admin-auction-table",
                            caption { class: "admin-visually-hidden", {t!("admin-recovery-title")} }
                            thead {
                                tr {
                                    th { scope: "col", {t!("admin-recovery-lot")} }
                                    th { scope: "col", {t!("admin-recovery-equipment")} }
                                    th { scope: "col", {t!("admin-recovery-stage")} }
                                    th { scope: "col", {t!("admin-recovery-deadline")} }
                                    th { scope: "col", {t!("admin-recovery-evidence")} }
                                    th { scope: "col", {t!("admin-recovery-actions")} }
                                    th { scope: "col", {t!("admin-recovery-commands")} }
                                }
                            }
                            tbody {
                                for recovery in rows {
                                    AdminRecoveryRow {
                                        recovery,
                                        action_busy,
                                        confirmation,
                                        reason,
                                        action_feedback,
                                    }
                                }
                            }
                        }
                    }
                }
                AdminSettlementPager {
                    current_page,
                    total_pages,
                    page,
                    label: t!("admin-recovery-pagination"),
                }
            }
        }
    }
}

#[component]
fn AdminRecoveryRow(
    recovery: AdminRecovery,
    action_busy: Signal<bool>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let recovery_id = recovery.id.clone();
    let display_equipment_name = recovery.equipment_name.clone();
    let display_equipment_identity = format!("{display_equipment_name}({})", recovery.world_id);
    let deadline_attribute = recovery.payment_deadline_at.clone();
    let deadline_text = super::super::format_admin_time(&recovery.payment_deadline_at);
    let lot_code =
        super::super::lots::admin_lot_code_for(&recovery.equipment_category, recovery.lot_number);
    let evidence = recovery
        .blocking_code
        .clone()
        .or(recovery.custody_proof_kind.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let actions = recovery_action_summary(&recovery.actions);
    rsx! {
        tr {
            td { "{lot_code}" }
            td {
                TruncatedText { value: display_equipment_identity, size: "wide", portal: true }
            }
            td { {recovery_stage_label(&recovery.stage)} }
            td { time { datetime: deadline_attribute, {deadline_text} } }
            td { TruncatedText { value: evidence, portal: true } }
            td { TruncatedText { value: actions, size: "wide", portal: true } }
            td {
                div { class: "admin-auction-actions",
                    for action in recovery.allowed_commands.clone() {
                        {
                            let draft = RecoveryConfirmationDraft {
                                auction_id: recovery.auction_id.clone(),
                                recovery_id: recovery_id.clone(),
                                state_version: recovery.state_version,
                                action: action.clone(),
                                equipment_name: recovery.equipment_name.clone(),
                                lot_code: lot_code.clone(),
                            };
                            rsx! {
                                button {
                                    class: "admin-btn-primary admin-auction-recovery-action",
                                    r#type: "button",
                                    disabled: action_busy(),
                                    onclick: move |_| {
                                        confirmation.set(Some(draft.clone()));
                                        reason.set(String::new());
                                        if action_feedback()
                                            .as_ref()
                                            .is_some_and(AdminActionFeedback::is_success)
                                        {
                                            action_feedback.set(None);
                                        }
                                    },
                                    {recovery_command_label(&action)}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AdminRecoveryDialog(
    draft: Option<RecoveryConfirmationDraft>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    mut command_ids: Signal<std::collections::HashMap<String, Uuid>>,
    selected_auction_id: Signal<String>,
    selection_generation: Signal<u64>,
    mut reload_generation: Signal<u64>,
    mut recovery_reload_generation: Signal<u64>,
) -> Element {
    let Some(draft) = draft else {
        return rsx! {};
    };
    let requires_reason = draft.action != "reclaimed";
    rsx! {
        div { class: "admin-shell__dialog-backdrop", role: "presentation",
            section {
                class: "admin-shell__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "settlement-recovery-confirm-title",
                aria_describedby: "settlement-recovery-confirm-description",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        event.stop_propagation();
                        confirmation.set(None);
                        reason.set(String::new());
                    }
                },
                h2 { id: "settlement-recovery-confirm-title", {recovery_command_label(&draft.action)} }
                p {
                    id: "settlement-recovery-confirm-description",
                    {t!("admin-recovery-confirm-description", lot: draft.lot_code.clone(), equipment: draft.equipment_name.clone())}
                }
                form {
                    class: "admin-auction-recovery-confirm",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() || (requires_reason && reason().trim().is_empty()) {
                            return;
                        }
                        busy.set(true);
                        let draft = draft.clone();
                        let reason_value = if draft.action == "reclaimed" {
                            String::new()
                        } else {
                            reason().trim().to_owned()
                        };
                        let command_signature = format!(
                            "{}:{}:{}:{reason_value}",
                            draft.recovery_id, draft.state_version, draft.action
                        );
                        let command_id = stable_command_id(
                            command_ids,
                            command_signature.clone(),
                        );
                        feedback.set(None);
                        let mut busy = busy;
                        let mut confirmation = confirmation;
                        let mut reason = reason;
                        let mut feedback = feedback;
                        let mut command_ids = command_ids;
                        let selected_auction_id = selected_auction_id;
                        let selection_generation = selection_generation;
                        let submitted_selection_generation = selection_generation();
                        let mut reload_generation = reload_generation;
                        let mut recovery_reload_generation = recovery_reload_generation;
                        spawn(async move {
                            let submitted_auction_id = draft.auction_id.clone();
                            let result = confirm_admin_recovery(
                                command_id,
                                draft.recovery_id,
                                draft.state_version,
                                draft.action,
                                reason_value,
                            )
                            .await;
                            let still_selected = selected_auction_id() == submitted_auction_id
                                && selection_generation() == submitted_selection_generation;
                            match result {
                                Ok(_) => {
                                    command_ids.write().remove(&command_signature);
                                    if still_selected {
                                        confirmation.set(None);
                                        reason.set(String::new());
                                        feedback.set(Some(AdminActionFeedback::Success));
                                        // 旧版成功后同时失效 auction、settlement 与 recovery；共享
                                        // generation 正好驱动这三个权威读取，不能只刷新当前表。
                                        reload_generation
                                            .set(reload_generation().saturating_add(1));
                                    }
                                }
                                Err(error) if error.is_conflict() => {
                                    if still_selected {
                                        // 旧版对全部 409 都关闭失效上下文，但保留错误和命令 ID 供同一动作重试。
                                        confirmation.set(None);
                                        reason.set(String::new());
                                        feedback.set(Some(AdminActionFeedback::Error(error.code)));
                                        recovery_reload_generation.set(
                                            recovery_reload_generation().saturating_add(1),
                                        );
                                    }
                                }
                                Err(error) => {
                                    if still_selected {
                                        // 400/503 保留对话框、输入和命令，便于同次重试。
                                        feedback.set(Some(AdminActionFeedback::Error(error.code)));
                                    }
                                }
                            }
                            busy.set(false);
                        });
                    },
                    if requires_reason {
                        label {
                            r#for: "admin-recovery-reason",
                            {t!("admin-recovery-reason")}
                            textarea {
                                id: "admin-recovery-reason",
                                rows: "4",
                                maxlength: "512",
                                required: true,
                                value: reason(),
                                oninput: move |event| reason.set(event.value()),
                            }
                        }
                    }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            class: "admin-btn-secondary admin-auction-recovery-action",
                            r#type: "button",
                            disabled: busy(),
                            autofocus: true,
                            onclick: move |_| {
                                confirmation.set(None);
                                reason.set(String::new());
                            },
                            {t!("admin-recovery-cancel")}
                        }
                        button {
                            class: "admin-btn-primary admin-auction-recovery-action",
                            r#type: "submit",
                            disabled: busy() || (requires_reason && reason().trim().is_empty()),
                            {t!("admin-recovery-confirm")}
                        }
                    }
                }
            }
        }
    }
}
