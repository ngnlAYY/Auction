//! 收件关联投递的安全展示与手动重排；原邮件处理结果和外部完成证明仍由后端拥有。

use auction_contracts::{AdminDeliveryActionBlocker, AdminDeliveryActionsResponse};
use auction_web_shared::{TableColumns, TruncatedText, t};
use dioxus::prelude::*;

use super::super::deliveries::dialog::{
    DeliveryConfirmationFacts, DeliveryEvidenceStatus, ReconcileEvidenceCandidates,
};
use super::super::deliveries::labels::{action_blocker_label, delivery_status_label};
use super::super::deliveries::modal::{DeliveryModal, close_delivery_modal};
use crate::api::{
    AdminMailIntakeDelivery, Delivery, DeliveryAction, execute_delivery, fetch_delivery_detail,
};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::{AdminPager as Pager, use_admin_pagination};
use crate::surface::pending::{PendingCommands, set_if_alive};

const DIALOG_ID: &str = "mail-delivery-action-dialog";

#[component]
pub(super) fn MailDeliveriesPanel(
    intake_id: String,
    deliveries: Vec<AdminMailIntakeDelivery>,
    mut notice: Signal<Option<(String, AdminActionFeedback)>>,
    refresh: Signal<u64>,
) -> Element {
    let mut page = use_signal(|| 1_i64);
    let pagination =
        use_admin_pagination("mail-deliveries", &["mail_delivery_page_size"], None, i64::MAX);
    let page_size = pagination.size();
    let commands = use_context::<PendingCommands>();
    let busy = move || commands.any_busy();
    let mut pending = use_signal(|| None::<AdminMailIntakeDelivery>);
    let confirm_intake_id = intake_id;
    let confirm_action =
        move |(delivery, action, operation): (Delivery, DeliveryAction, String)| {
            if busy() {
                return;
            }
            if commands.contains(&delivery.id)
                || pending().is_none_or(|row| {
                    row.id != delivery.id || row.fencing_version != delivery.fencing_version
                })
            {
                return;
            }
            let success_intake_id = confirm_intake_id.clone();
            let failure_intake_id = confirm_intake_id.clone();
            notice.set(None);
            let submitted = commands.submit(
                delivery.id.clone(),
                retry_label(&operation),
                move |command_id| {
                    let delivery_id = delivery.id.clone();
                    async move {
                        execute_delivery(
                            command_id,
                            delivery_id,
                            action,
                            delivery.fencing_version,
                            None,
                            None,
                        )
                        .await
                        .map_err(|error| error.code)
                    }
                },
                move |_| {
                    set_if_alive(
                        notice,
                        Some((success_intake_id.clone(), AdminActionFeedback::Success)),
                    );
                },
                move |code| {
                    if matches!(
                        code.as_str(),
                        "stale_delivery_state"
                            | "stale_state_version"
                            | "delivery_recipient_renamed"
                    ) {
                        let mut refresh = refresh;
                        refresh.set(refresh().saturating_add(1));
                    }
                    set_if_alive(
                        notice,
                        Some((failure_intake_id.clone(), AdminActionFeedback::Error(code))),
                    );
                },
            );
            if submitted {
                close_delivery_modal(DIALOG_ID);
                pending.set(None);
            }
        };
    let total = i64::try_from(deliveries.len()).unwrap_or(i64::MAX);
    let current_page = page().clamp(1, if total == 0 { 1 } else { 1 + (total - 1) / page_size });
    let offset =
        usize::try_from((current_page - 1).saturating_mul(page_size)).unwrap_or(usize::MAX);
    let visible =
        deliveries.into_iter().skip(offset).take(usize::try_from(page_size).unwrap_or(usize::MAX));

    rsx! {
        section { class: "admin-mail-deliveries", aria_labelledby: "admin-mail-deliveries-title",
            h3 { id: "admin-mail-deliveries-title", {t!("admin-mail-deliveries-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-deliveries-help")} }
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table admin-mail-deliveries__table",
                    aria_label: t!("admin-mail-deliveries-title"),
                    // 操作类型、收件人、任务 ID、状态、尝试、失败原因、操作合计 100%。
                    TableColumns { layout: "admin-mail-deliveries", widths: vec![12, 14, 20, 14, 6, 19, 15] }
                    thead {
                        tr {
                            th { scope: "col", {t!("admin-mail-delivery-operation")} }
                            th { scope: "col", {t!("delivery-to")} }
                            th { scope: "col", {t!("admin-mail-delivery-id")} }
                            th { scope: "col", {t!("delivery-status")} }
                            th { scope: "col", {t!("delivery-attempts")} }
                            th { scope: "col", {t!("delivery-failure-code")} }
                            th { scope: "col", {t!("catalog-actions")} }
                        }
                    }
                    tbody {
                        for delivery in visible {
                            MailDeliveryRow {
                                key: "{delivery.id}",
                                busy: busy() || commands.contains(&delivery.id),
                                delivery,
                                on_retry: move |delivery: AdminMailIntakeDelivery| {
                                    if !busy() && !commands.contains(&delivery.id) {
                                        notice.set(None);
                                        pending.set(Some(delivery));
                                    }
                                },
                            }
                        }
                    }
                }
            }
            if total == 0 {
                p { class: "admin-panel__note", {t!("admin-mail-deliveries-empty")} }
            }
            Pager {
                    pagination,
                    local: true,
                    page: page(),
                    page_size,
                    total: Some(total),
                    label: t!("admin-mail-deliveries-title"),
                    on_page_change: move |next| page.set(next),
            }
            if let Some(delivery) = pending() {
                MailRetryDialog {
                    delivery,
                    busy: busy(),
                    on_submit: confirm_action,
                    on_cancel: move |_| pending.set(None),
                }
            }
        }
    }
}

#[component]
fn MailRetryDialog(
    delivery: AdminMailIntakeDelivery,
    busy: bool,
    on_submit: EventHandler<(Delivery, DeliveryAction, String)>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut resource = use_resource(use_reactive((&delivery.id,), move |(delivery_id,)| {
        fetch_delivery_detail(delivery_id)
    }));
    let result =
        if resource.state().cloned() == UseResourceState::Ready { resource.cloned() } else { None };
    let evidence_status = match &result {
        None => Some(t!("delivery-reconcile-sightings-loading")),
        Some(Ok(projected))
            if projected.detail.delivery.fencing_version == delivery.fencing_version
                && retry_action(projected.actions) == Ok(DeliveryAction::ReconcileRetry)
                && projected.detail.evidence.is_empty() =>
        {
            Some(t!("delivery-reconcile-sightings-empty"))
        }
        _ => None,
    };
    rsx! {
        DeliveryModal {
            dialog_id: DIALOG_ID,
            title: retry_label(&delivery.operation),
            busy,
            on_cancel,
            DeliveryEvidenceStatus { message: evidence_status }
            div {
                match result {
                    None => rsx! {},
                    Some(Err(_error)) => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert", {t!("delivery-reconcile-sightings-error")} }
                        button { r#type: "button", disabled: busy, onclick: move |_| resource.restart(), {t!("admin-retry")} }
                    },
                    Some(Ok(projected)) if projected.detail.delivery.fencing_version != delivery.fencing_version => rsx! {
                        p { class: "action-feedback action-feedback--error", role: "alert", {t!("delivery-action-blocked-stale")} }
                    },
                    Some(Ok(projected)) => {
                        let target = projected.detail.delivery;
                        let operation = delivery.operation;
                        match retry_action(projected.actions) {
                            Ok(action) => rsx! {
                                DeliveryConfirmationFacts { delivery: target.clone() }
                                if action == DeliveryAction::ReconcileRetry {
                                    ReconcileEvidenceCandidates { evidence: projected.detail.evidence }
                                    p { class: "admin-mail-deliveries__warning", {t!("admin-mail-delivery-confirm-unknown")} }
                                }
                                button {
                                    r#type: "button",
                                    class: "admin-btn-primary",
                                    disabled: busy,
                                    onclick: move |_| on_submit.call((target.clone(), action, operation.clone())),
                                    {t!("delivery-confirm")}
                                }
                            },
                            Err(blocker) => rsx! { p { class: "admin-panel__note", {action_blocker_label(blocker)} } },
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn MailDeliveryRow(
    delivery: AdminMailIntakeDelivery,
    busy: bool,
    on_retry: EventHandler<AdminMailIntakeDelivery>,
) -> Element {
    let target = delivery.clone();
    rsx! {
        tr {
            td { TruncatedText { value: operation_label(&delivery.operation) } }
            td { TruncatedText { value: delivery.recipient } }
            td { TruncatedText { value: delivery.id } }
            td { span { class: "admin-delivery-badge", {delivery_status_label(&delivery.status)} } }
            td { "{delivery.attempt_count}" }
            td { TruncatedText { value: delivery.failure_code.unwrap_or_else(|| t!("value-empty")) } }
            td {
                button {
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_retry.call(target.clone()),
                    {t!("delivery-review-actions")}
                }
            }
        }
    }
}

fn retry_action(
    actions: AdminDeliveryActionsResponse,
) -> Result<DeliveryAction, AdminDeliveryActionBlocker> {
    use AdminDeliveryActionBlocker::StatusMismatch;

    match (actions.retry_failed.blocker(), actions.reconcile_retry.blocker()) {
        (None, Some(_)) => Ok(DeliveryAction::Retry),
        (Some(_), None) => Ok(DeliveryAction::ReconcileRetry),
        (Some(StatusMismatch), Some(reason)) | (Some(reason), Some(_)) => Err(reason),
        (None, None) => Err(StatusMismatch),
    }
}

fn operation_label(operation: &str) -> String {
    match operation {
        "send" => t!("admin-mail-delivery-send"),
        "return" => t!("admin-mail-delivery-return"),
        "take" => t!("admin-mail-delivery-take"),
        _ => t!("value-empty"),
    }
}

fn retry_label(operation: &str) -> String {
    match operation {
        "send" => t!("admin-mail-delivery-resend"),
        "return" => t!("admin-mail-delivery-return-again"),
        "take" => t!("admin-mail-delivery-take-again"),
        _ => t!("value-empty"),
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::{
        AdminDeliveryActionAvailability, AdminDeliveryActionBlocker, AdminDeliveryActionsResponse,
    };

    use super::retry_action;
    use crate::api::DeliveryAction;

    #[test]
    fn linked_mail_uses_the_fence_decision_for_retry() {
        let mismatch = AdminDeliveryActionAvailability::Blocked {
            reason: AdminDeliveryActionBlocker::StatusMismatch,
        };
        let mut actions = AdminDeliveryActionsResponse {
            release: mismatch,
            retry_failed: mismatch,
            reconcile_retry: AdminDeliveryActionAvailability::Eligible {},
            reconcile_succeeded: mismatch,
            reconcile_failed: mismatch,
            acknowledge: mismatch,
        };
        assert_eq!(retry_action(actions), Ok(DeliveryAction::ReconcileRetry));
        actions.reconcile_retry = AdminDeliveryActionAvailability::Blocked {
            reason: AdminDeliveryActionBlocker::SettlementReturnActionOwned,
        };
        assert_eq!(
            retry_action(actions),
            Err(AdminDeliveryActionBlocker::SettlementReturnActionOwned)
        );
    }
}
