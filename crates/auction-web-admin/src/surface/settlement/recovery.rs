//! Admin recovery 表格、命令确认对话框和幂等提交交互。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};

use crate::api::{AdminRecovery, AdminRecoveryReport, confirm_admin_recovery};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::AdminPagination;
use crate::surface::pending::{PendingCommands, set_if_alive};

use super::RecoveryConfirmationDraft;
use super::helpers::{recovery_action_summary, recovery_command_label, recovery_stage_label};
use super::report::AdminSettlementPager;

#[component]
pub(super) fn AdminRecoveryTable(
    report: Option<AdminRecoveryReport>,
    current_page: i64,
    pagination: AdminPagination,
    mut page: Signal<i64>,
    action_busy: Signal<bool>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let rows = report
        .as_ref()
        .map(|report| {
            super::helpers::page_slice(&report.recoveries, current_page, pagination.size())
        })
        .unwrap_or_default();
    rsx! {
        section { class: "admin-auction-report admin-auction-recovery", aria_labelledby: "settlement-recovery-title",
            h2 { id: "settlement-recovery-title", {t!("admin-recovery-title")} }
            if let Some(report) = report {
                if report.recoveries.is_empty() {
                    AsyncState {
                        kind: AsyncStateKind::Empty,
                        title: t!("admin-recovery-empty"),
                        detail: "",
                    }
                } else {
                    div { class: "admin-settlement__table-wrap",
                        table { class: "admin-shell__table admin-auction-table",
                            caption { class: "admin-visually-hidden", {t!("admin-recovery-title")} }
                            auction_web_shared::TableColumns { layout: "admin-recovery", widths: vec![6, 27, 10, 18, 14, 12, 13] }
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
                    current_page: page(),
                    total_rows: report.recoveries.len(),
                    pagination,
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
    let pending = use_context::<PendingCommands>();
    let recovery_id = recovery.id.clone();
    let display_equipment_name = recovery.equipment_name.clone();
    let display_equipment_identity = format!("{display_equipment_name}({})", recovery.world_id);
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
            td { Time { value: recovery.payment_deadline_at.clone() } }
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
                                    disabled: action_busy() || pending.contains(&recovery_id),
                                    onclick: move |_| {
                                        if pending.any_busy() || pending.contains(&draft.recovery_id) {
                                            return;
                                        }
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
    busy: Signal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    selected_auction_id: Signal<String>,
    selection_generation: Signal<u64>,
    mut recovery_reload_generation: Signal<u64>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let Some(draft) = draft else {
        return rsx! {};
    };
    let requires_reason = draft.action != "reclaimed";
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；共享组件暂不支持 aria_describedby，
    // 描述段落保留在内容区首部。
    rsx! {
        Dialog {
            title: recovery_command_label(&draft.action),
            close_label: t!("admin-recovery-cancel"),
            busy: busy(),
            on_close: move |_| {
                confirmation.set(None);
                reason.set(String::new());
            },
            p {
                {t!("admin-recovery-confirm-description", lot: draft.lot_code.clone(), equipment: draft.equipment_name.clone())}
            }
            form {
                    class: "admin-auction-recovery-confirm",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if pending.any_busy() || pending.contains(&draft.recovery_id)
                            || (requires_reason && reason().trim().is_empty())
                        {
                            return;
                        }
                        let draft = draft.clone();
                        let reason_value = if draft.action == "reclaimed" {
                            String::new()
                        } else {
                            reason().trim().to_owned()
                        };
                        let submitted_selection_generation = selection_generation();
                        let success_auction_id = draft.auction_id.clone();
                        let failure_auction_id = draft.auction_id.clone();
                        let submitted = pending.submit(
                            draft.recovery_id.clone(),
                            recovery_command_label(&draft.action),
                            move |command_id| {
                                let draft = draft.clone();
                                let reason_value = reason_value.clone();
                                async move {
                                    let result = confirm_admin_recovery(
                                        command_id,
                                        draft.recovery_id,
                                        draft.state_version,
                                        draft.action,
                                        reason_value,
                                    ).await;
                                    if result.as_ref().is_err_and(|error| error.is_conflict())
                                        && selected_auction_id() == draft.auction_id
                                        && selection_generation() == submitted_selection_generation
                                    {
                                        recovery_reload_generation.set(
                                            recovery_reload_generation().saturating_add(1),
                                        );
                                    }
                                    result.map_err(|error| error.code)
                                }
                            },
                            move |_| {
                                if selected_auction_id() == success_auction_id
                                    && selection_generation() == submitted_selection_generation
                                {
                                    set_if_alive(feedback, Some(AdminActionFeedback::Success));
                                }
                            },
                            move |code| {
                                if selected_auction_id() == failure_auction_id
                                    && selection_generation() == submitted_selection_generation
                                {
                                    set_if_alive(feedback, Some(AdminActionFeedback::Error(code)));
                                }
                            },
                        );
                        if submitted {
                            // 完整意图归页级宿主；关闭展示不能修改未知命令的原因、版本或 UUID。
                            confirmation.set(None);
                            reason.set(String::new());
                            feedback.set(None);
                        }
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
