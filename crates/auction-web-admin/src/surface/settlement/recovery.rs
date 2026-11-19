//! Admin recovery 表格、命令确认对话框和幂等提交交互。

use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::presentation::{
    recovery_action_summary, recovery_command_label, recovery_stage_label,
    send_attribution_blocker_label,
};
use super::query::read_settlement_query;
use super::report::AdminSettlementPager;
use crate::api::{AdminRecovery, AdminRecoveryReport, confirm_admin_recovery};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::AdminPagination;
use crate::surface::pending::{PendingCommands, set_if_alive};
use crate::surface::validation::{FieldError, focus_field};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RecoveryConfirmationDraft {
    pub(super) auction_id: String,
    pub(super) recovery_id: String,
    pub(super) state_version: i64,
    pub(super) action: String,
    pub(super) equipment_name: String,
    pub(super) lot_code: String,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct RecoveryRead {
    pub(super) reload: Signal<u64>,
    pub(super) recovery_reload: Signal<u64>,
    pub(super) generation: (u64, u64),
}

impl RecoveryRead {
    fn is_current(self) -> bool {
        ((self.reload)(), (self.recovery_reload)()) == self.generation
    }
}

#[component]
pub(super) fn AdminRecoveryTable(
    report: Option<AdminRecoveryReport>,
    current_page: i64,
    pagination: AdminPagination,
    mut page: Signal<i64>,
    action_busy: ReadSignal<bool>,
    recovery_read: RecoveryRead,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    mut action_feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let rows = report.as_ref().map_or_default(|report| {
        super::presentation::page_slice(&report.recoveries, current_page, pagination.size())
    });
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
                            aria_label: t!("admin-recovery-title"),
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
                                        recovery_read,
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
    action_busy: ReadSignal<bool>,
    recovery_read: RecoveryRead,
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
    // 发送归属不成立时显示具体原因（Worker 阻断与 Admin 确认共用同一判定），否则沿用阻断码或
    // 收回证明。
    let evidence = recovery
        .send_attribution_blocker
        .as_deref()
        .map(send_attribution_blocker_label)
        .or_else(|| recovery.blocking_code.clone())
        .or_else(|| recovery.custody_proof_kind.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let actions = recovery_action_summary(&recovery.actions);
    rsx! {
        tr {
            td { "data-label": t!("admin-recovery-lot"), "{lot_code}" }
            td { "data-label": t!("admin-recovery-equipment"),
                TruncatedText { value: display_equipment_identity, size: "wide", portal: true }
            }
            td { "data-label": t!("admin-recovery-stage"), {recovery_stage_label(&recovery.stage)} }
            td { "data-label": t!("admin-recovery-deadline"), Time { value: recovery.payment_deadline_at.clone() } }
            td { "data-label": t!("admin-recovery-evidence"), TruncatedText { value: evidence, portal: true } }
            td { "data-label": t!("admin-recovery-actions"), TruncatedText { value: actions, size: "wide", portal: true } }
            td { "data-label": t!("admin-recovery-commands"),
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
                                    disabled: !recovery_read.is_current() || action_busy()
                                        || pending.contains(&format!("settlement-recovery:{recovery_id}")),
                                    onclick: move |_| {
                                        if !recovery_read.is_current() || read_settlement_query().is_err()
                                            || pending.any_busy() || pending.contains(&format!("settlement-recovery:{}", draft.recovery_id)) {
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
    draft: RecoveryConfirmationDraft,
    mut confirmation: Signal<Option<RecoveryConfirmationDraft>>,
    mut reason: Signal<String>,
    busy: ReadSignal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    selected_auction_id: Signal<String>,
    selection_generation: Signal<u64>,
    recovery_read: RecoveryRead,
    mut recovery_reload_generation: Signal<u64>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let mut reason_issue = use_signal(|| None::<RecoveryReasonIssue>);
    let generation = selection_generation();
    let requires_reason = draft.action != "reclaimed";
    let shows_refund = activates_buyer_refund(&draft.action);
    let blocked = pending.contains(&format!("settlement-recovery:{}", draft.recovery_id));
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
                reason_issue.set(None);
            },
            p {
                {t!("admin-recovery-confirm-description", lot: draft.lot_code.clone(), equipment: draft.equipment_name.clone())}
            }
            if shows_refund {
                p { {t!("admin-recovery-confirm-refund-description")} }
            }
            form {
                    class: "admin-auction-recovery-confirm",
                    novalidate: true,
                    onsubmit: move |event| {
                        event.prevent_default();
                        // 首次提交只作用于打开确认时选中的场次，重试继续使用冻结的意图。
                        if !recovery_read.is_current() || read_settlement_query().is_err()
                            || pending.any_busy() || pending.contains(&format!("settlement-recovery:{}", draft.recovery_id))
                            || selected_auction_id() != draft.auction_id
                            || selection_generation() != generation
                        {
                            return;
                        }
                        let reason_value = match validate_recovery_reason(&draft.action, &reason()) {
                            Ok(value) => value.to_owned(),
                            Err(error) => {
                                reason_issue.set(Some(error));
                                focus_field("admin-recovery-reason");
                                return;
                            }
                        };
                        let draft = draft.clone();
                        let submitted_selection_generation = generation;
                        let success_auction_id = draft.auction_id.clone();
                        let failure_auction_id = draft.auction_id.clone();
                        let submitted = pending.submit_with_receipt(
                            format!("settlement-recovery:{}", draft.recovery_id),
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
                                    if result.as_ref().is_err_and(crate::api::AdminApiFailure::is_conflict)
                                        && same_recovery_selection(
                                            selected_auction_id, selection_generation,
                                            &draft.auction_id, submitted_selection_generation,
                                        )
                                        && let Ok(mut reload) = recovery_reload_generation.try_write()
                                    {
                                        *reload = reload.saturating_add(1);
                                    }
                                    result.map_err(|error| error.code)
                                }
                            },
                            move |_, source_alive| {
                                let delivered = source_alive && same_recovery_selection(
                                    selected_auction_id, selection_generation,
                                    &success_auction_id, submitted_selection_generation,
                                ) && crate::surface::pending::try_set_if_alive(feedback, Some(AdminActionFeedback::Success));
                                crate::surface::pending::CommandDelivery::reported(delivered, || None)
                            },
                            move |code| {
                                if same_recovery_selection(
                                    selected_auction_id, selection_generation,
                                    &failure_auction_id, submitted_selection_generation,
                                ) {
                                    set_if_alive(feedback, Some(AdminActionFeedback::Error(code)));
                                }
                            },
                        );
                        if submitted {
                            // 完整意图归认证外壳；关闭展示不能修改未知命令的原因、版本或 UUID。
                            confirmation.set(None);
                            reason.set(String::new());
                            reason_issue.set(None);
                            feedback.set(None);
                        }
                    },
                    if requires_reason {
                        label {
                            r#for: "admin-recovery-reason",
                            {t!("admin-recovery-reason")}
                            textarea {
                                id: "admin-recovery-reason",
                                name: "reason",
                                rows: "4",
                                required: true,
                                value: reason(),
                                aria_invalid: reason_issue().is_some().to_string(),
                                aria_describedby: reason_issue().is_some().then_some("admin-recovery-reason-error"),
                                oninput: move |event| {
                                    reason.set(event.value());
                                    reason_issue.set(None);
                                },
                            }
                        }
                        FieldError {
                            id: "admin-recovery-reason-error",
                            message: reason_issue().map(RecoveryReasonIssue::message),
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
                                reason_issue.set(None);
                            },
                            {t!("admin-recovery-cancel")}
                        }
                        button {
                            class: "admin-btn-primary admin-auction-recovery-action",
                            r#type: "submit",
                            disabled: busy() || blocked || !recovery_read.is_current(),
                            {t!("admin-recovery-confirm")}
                        }
                    }
            }
        }
    }
}

/// 无效链接会卸载选择视图。已提交命令的回执不得再读取销毁的视图状态。
fn same_recovery_selection(
    auction: Signal<String>,
    generation: Signal<u64>,
    expected_auction: &str,
    expected_generation: u64,
) -> bool {
    auction.try_peek().is_ok_and(|value| value.as_str() == expected_auction)
        && generation.try_peek().is_ok_and(|value| *value == expected_generation)
}

// 恢复确认应用端口检查 trim 后将要提交的理由，内部换行是合法输入。
const MAX_RECOVERY_REASON_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryReasonIssue {
    Required,
    TooLong,
}

impl RecoveryReasonIssue {
    fn message(self) -> String {
        match self {
            Self::Required => t!("admin-field-required"),
            Self::TooLong => t!("admin-field-max-utf8-bytes", max: MAX_RECOVERY_REASON_BYTES),
        }
    }
}

fn validate_recovery_reason<'a>(
    action: &str,
    reason: &'a str,
) -> Result<&'a str, RecoveryReasonIssue> {
    if action == "reclaimed" {
        return Ok("");
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(RecoveryReasonIssue::Required);
    }
    if reason.len() > MAX_RECOVERY_REASON_BYTES {
        return Err(RecoveryReasonIssue::TooLong);
    }
    Ok(reason)
}

fn activates_buyer_refund(action: &str) -> bool {
    action == "equipment_recovered"
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use dioxus::dioxus_core::{AttributeValue, Mutation};
    use dioxus::prelude::*;

    use super::{AdminRecoveryRow, RecoveryRead};
    use super::{RecoveryReasonIssue, activates_buyer_refund, validate_recovery_reason};
    use crate::api::AdminRecovery;
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, has_live_text, init_i18n, mount_writer,
    };
    use crate::surface::pending::{use_pending_command_host, use_pending_commands};

    fn recovery_row() -> Element {
        init_i18n();
        let mut reload = use_signal(|| 0);
        let mut loaded_generation = use_signal(|| 0);
        use_pending_command_host();
        use_pending_commands(reload, |_| true);
        let action_busy = use_signal(|| false);
        let confirmation = use_signal(|| None);
        let reason = use_signal(String::new);
        let action_feedback = use_signal(|| None);
        let recovery_read = RecoveryRead {
            reload,
            recovery_reload: reload,
            generation: (loaded_generation(), loaded_generation()),
        };
        let recovery: AdminRecovery = serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001", "world_id": "main",
            "auction_id": "00000000-0000-0000-0000-000000000002", "lot_number": 1,
            "lot_id": "00000000-0000-0000-0000-000000000003", "equipment_name": "Phase Robe",
            "equipment_category": "Cloth", "stage": "reclaiming", "resolution": null,
            "state_version": 2, "payment_deadline_at": "2027-01-01T00:00:00Z",
            "reminder_count": 0, "reminder_limit": 1, "custody_proof_kind": null,
            "custody_proof_observed_at": null, "blocking_code": null,
            "manual_reconcile_delivery_id": null, "relisted_auction_id": null, "relisted_lot_id": null,
            "actions": [], "allowed_commands": ["reclaimed"], "updated_at": "2027-01-01T00:00:00Z"
        })).unwrap();
        rsx! {
            button { onclick: move |_| reload.set(1), {"使恢复报告失效".to_owned()} }
            button { onclick: move |_| loaded_generation.set(1), {"完成恢复报告读取".to_owned()} }
            AdminRecoveryRow { recovery, action_busy, recovery_read, confirmation, reason, action_feedback }
            if confirmation().is_some() { p { {"已有恢复确认草稿".to_owned()} } }
        }
    }

    #[test]
    fn every_recovery_cell_renders_its_card_label() {
        let mutations = VirtualDom::new(recovery_row).rebuild_to_vec();
        let labels = mutations
            .edits
            .iter()
            .filter_map(|edit| match edit {
                Mutation::SetAttribute {
                    name: "data-label",
                    value: AttributeValue::Text(value),
                    ..
                } => Some(value.as_str()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            labels,
            BTreeSet::from([
                "拍品",
                "装备",
                "恢复阶段",
                "付款截止",
                "证据/阻塞原因",
                "恢复操作",
                "操作"
            ])
        );
    }

    #[test]
    fn recovery_entry_rejects_events_from_an_invalidated_report() {
        let (mut page, _) = mount_writer(recovery_row);
        assert!(!button(&page, "确认已回收").unwrap().1);
        click_button(&mut page, "使恢复报告失效");
        let (stale, disabled) = button(&page, "确认已回收").unwrap();
        assert!(disabled);
        page.dispatch("click", stale, "");
        assert!(!has_live_text(&page, "已有恢复确认草稿"));
        click_button(&mut page, "完成恢复报告读取");
        click_button(&mut page, "确认已回收");
        assert!(has_live_text(&page, "已有恢复确认草稿"));
    }

    #[test]
    fn recovery_reasons_count_utf8_bytes_and_allow_internal_newlines() {
        let limit = "拍".repeat(170) + "ab";
        assert_eq!(validate_recovery_reason("cancelled", &limit), Ok(limit.as_str()));
        assert_eq!(
            validate_recovery_reason("equipment_recovered", &"拍".repeat(171)),
            Err(RecoveryReasonIssue::TooLong)
        );
        assert_eq!(validate_recovery_reason("cancelled", " 原因\n第二行 "), Ok("原因\n第二行"));
        assert_eq!(validate_recovery_reason("cancelled", " "), Err(RecoveryReasonIssue::Required));
        assert_eq!(validate_recovery_reason("reclaimed", ""), Ok(""));
    }

    #[test]
    fn refund_consequences_apply_only_to_recovered_equipment() {
        assert!(activates_buyer_refund("equipment_recovered"));
        assert!(!activates_buyer_refund("reclaimed"));
        assert!(!activates_buyer_refund("cancelled"));
    }
}
