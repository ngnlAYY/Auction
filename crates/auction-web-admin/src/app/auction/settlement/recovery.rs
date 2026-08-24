//! Admin recovery 表格、命令确认对话框和幂等提交交互。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminRecovery, AdminRecoveryReport, confirm_admin_recovery};

use super::RecoveryConfirmationDraft;
use super::helpers::{
    recovery_action_summary, recovery_command_label, recovery_stage_class, recovery_stage_label,
    stable_command_id,
};
use super::report::AdminSettlementPager;

#[component]
pub(super) fn AdminRecoveryTable(
    report: AdminRecoveryReport,
    current_page: i64,
    total_pages: usize,
    mut page: Signal<i64>,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let rows = super::helpers::page_slice(&report.recoveries, current_page);
    rsx! {
        section { class: "admin-settlement__recovery", aria_labelledby: "admin-recovery-title",
            h2 { id: "admin-recovery-title", {t!("admin-recovery-title")} }
            if report.recoveries.is_empty() {
                p { class: "admin-panel__note", {t!("admin-recovery-empty")} }
            } else {
                div { class: "admin-settlement__table-wrap",
                    table {
                        caption { class: "admin-visually-hidden", {t!("admin-recovery-title")} }
                        thead {
                            tr {
                                th { {t!("admin-recovery-lot")} }
                                th { {t!("admin-recovery-equipment")} }
                                th { {t!("admin-recovery-stage")} }
                                th { {t!("admin-recovery-deadline")} }
                                th { {t!("admin-recovery-evidence")} }
                                th { {t!("admin-recovery-actions")} }
                                th { {t!("admin-recovery-commands")} }
                            }
                        }
                        tbody {
                            for recovery in rows {
                                AdminRecoveryRow { recovery, confirmation, reason, action_feedback }
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

#[component]
fn AdminRecoveryRow(
    recovery: AdminRecovery,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<bool>>,
) -> Element {
    let recovery_id = recovery.id.clone();
    let display_equipment_name = recovery.equipment_name.clone();
    let deadline_attribute = recovery.payment_deadline_at.clone();
    let deadline_text = super::super::format_admin_time(&recovery.payment_deadline_at);
    let evidence = recovery
        .blocking_code
        .clone()
        .or(recovery.custody_proof_kind.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let actions = recovery_action_summary(&recovery.actions);
    rsx! {
        tr { class: "admin-settlement__recovery-row",
            td { "data-label": t!("admin-recovery-lot"), "{recovery.lot_number}" }
            td { "data-label": t!("admin-recovery-equipment"),
                TruncatedText { value: display_equipment_name }
                small { TruncatedText { value: recovery.world_id.clone(), size: "compact" } }
            }
            td { "data-label": t!("admin-recovery-stage"),
                span { class: recovery_stage_class(&recovery.stage), {recovery_stage_label(&recovery.stage)} }
            }
            td { "data-label": t!("admin-recovery-deadline"), time { datetime: deadline_attribute, {deadline_text} } }
            td { "data-label": t!("admin-recovery-evidence"), TruncatedText { value: evidence, size: "wide" } }
            td { "data-label": t!("admin-recovery-actions"), TruncatedText { value: actions, size: "wide" } }
            td { "data-label": t!("admin-recovery-commands"),
                div { class: "admin-settlement__commands",
                    for action in recovery.allowed_commands.clone() {
                        {
                            let draft = RecoveryConfirmationDraft {
                                recovery_id: recovery_id.clone(),
                                state_version: recovery.state_version,
                                action: action.clone(),
                                equipment_name: recovery.equipment_name.clone(),
                                lot_number: recovery.lot_number,
                            };
                            rsx! {
                                button {
                                    class: "admin-btn-primary",
                                    r#type: "button",
                                    onclick: move |_| {
                                        confirmation.set(Some(draft.clone()));
                                        reason.set(String::new());
                                        action_feedback.set(None);
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
    draft: RecoveryConfirmationDraft,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut reload_generation: Signal<u64>,
) -> Element {
    let requires_reason = draft.action != "reclaimed";
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    rsx! {
        div { class: "admin-settlement__dialog-backdrop", role: "presentation",
            section {
                class: "admin-settlement__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-recovery-dialog-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        confirmation.set(None);
                        reason.set(String::new());
                        feedback.set(None);
                    }
                },
                h2 { id: "admin-recovery-dialog-title", {recovery_command_label(&draft.action)} }
                p { {t!("admin-recovery-confirm-description", lot: draft.lot_number, equipment: draft.equipment_name.clone())} }
                form {
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
                        let command_id = stable_command_id(
                            command_state,
                            format!(
                                "{}:{}:{}:{reason_value}",
                                draft.recovery_id, draft.state_version, draft.action
                            ),
                        );
                        let mut busy = busy;
                        let mut confirmation = confirmation;
                        let mut reason = reason;
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        spawn(async move {
                            match confirm_admin_recovery(
                                command_id,
                                draft.recovery_id,
                                draft.state_version,
                                draft.action,
                                reason_value,
                            )
                            .await
                            {
                                Ok(_) => {
                                    confirmation.set(None);
                                    reason.set(String::new());
                                    feedback.set(Some(true));
                                    command_state.set(None);
                                    reload_generation
                                        .set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
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
                                disabled: busy(),
                                value: reason(),
                                oninput: move |event| reason.set(event.value()),
                            }
                        }
                    }
                    div { class: "admin-panel__actions",
                        button {
                            class: "admin-btn-secondary",
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
                            class: "admin-btn-primary",
                            r#type: "submit",
                            disabled: busy() || (requires_reason && reason().trim().is_empty()),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-recovery-confirm")} }
                        }
                    }
                }
            }
        }
    }
}
