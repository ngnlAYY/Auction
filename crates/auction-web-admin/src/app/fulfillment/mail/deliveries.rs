//! 收件关联投递的安全展示与手动重排；原邮件处理结果和外部完成证明仍由后端拥有。

use auction_web_shared::{Dialog, Pager, TableColumns, TruncatedText, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{AdminMailIntakeDelivery, DeliveryAction, TABLE_PAGE_SIZE, execute_delivery};
use crate::app::surface::{AdminActionFeedback, run_admin_action, stable_command_id};

use super::super::deliveries::helpers::delivery_status_label;

#[component]
pub(super) fn MailDeliveriesPanel(
    intake_id: String,
    deliveries: Vec<AdminMailIntakeDelivery>,
    mut notice: Signal<Option<(String, AdminActionFeedback)>>,
    mut refresh: Signal<u64>,
) -> Element {
    let mut page = use_signal(|| 1_i64);
    let busy = use_signal(|| false);
    let mut pending = use_signal(|| None::<AdminMailIntakeDelivery>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let confirm_intake_id = intake_id.clone();
    let confirm_action = move |_| {
        if busy() {
            return;
        }
        let Some(delivery) = pending() else {
            return;
        };
        let Some(action) = retry_action(&delivery) else {
            return;
        };
        let command_id = stable_command_id(
            command_state,
            format!("{}:{action:?}:{}", delivery.id, delivery.fencing_version),
        );
        let success_intake_id = confirm_intake_id.clone();
        let failure_intake_id = confirm_intake_id.clone();
        notice.set(None);
        run_admin_action(
            busy,
            async move {
                execute_delivery(
                    command_id,
                    delivery.id,
                    action,
                    delivery.fencing_version,
                    None,
                    None,
                )
                .await
            },
            move |_| {
                pending.set(None);
                command_state.set(None);
                notice.set(Some((success_intake_id, AdminActionFeedback::Success)));
                refresh.set(refresh().saturating_add(1));
            },
            move |error| {
                if error.is_conflict()
                    && matches!(error.code.as_str(), "stale_delivery_state" | "stale_state_version")
                {
                    // 版本冲突必须重新读取任务，不能用旧 fence 或新命令绕过并发校验。
                    pending.set(None);
                    command_state.set(None);
                    refresh.set(refresh().saturating_add(1));
                }
                notice.set(Some((failure_intake_id, AdminActionFeedback::Error(error.code))));
            },
        );
    };
    let total = i64::try_from(deliveries.len()).unwrap_or(i64::MAX);
    let current_page = page().clamp(1, ((total + TABLE_PAGE_SIZE - 1) / TABLE_PAGE_SIZE).max(1));
    let offset = usize::try_from((current_page - 1) * TABLE_PAGE_SIZE).unwrap_or(usize::MAX);
    let visible = deliveries.into_iter().skip(offset).take(TABLE_PAGE_SIZE as usize);
    let dialog_error = notice().and_then(|(source, feedback)| {
        (source == intake_id && !feedback.is_success()).then_some(feedback)
    });

    rsx! {
        section { class: "admin-mail-deliveries", aria_labelledby: "admin-mail-deliveries-title",
            h3 { id: "admin-mail-deliveries-title", {t!("admin-mail-deliveries-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-deliveries-help")} }
            div { class: "admin-shell__table-wrap",
                table { class: "admin-shell__table admin-mail-deliveries__table",
                    caption { class: "admin-visually-hidden", {t!("admin-mail-deliveries-title")} }
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
                                delivery,
                                busy: busy(),
                                on_retry: move |delivery| {
                                    if !busy() {
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
            if total > TABLE_PAGE_SIZE {
                Pager {
                    page: current_page,
                    page_size: TABLE_PAGE_SIZE,
                    total: Some(total),
                    label: t!("admin-mail-deliveries-title"),
                    previous_label: t!("admin-pagination-previous"),
                    status: move |(page, pages): (i64, i64)| {
                        t!("admin-pagination-status", page: page, pages: pages)
                    },
                    next_label: t!("admin-pagination-next"),
                    jump_label: t!("admin-pagination-input"),
                    jump_button_label: t!("admin-pagination-go"),
                    on_page_change: move |next| page.set(next),
                }
            }
            if let Some(delivery) = pending() {
                Dialog {
                    key: "mail-delivery-{delivery.id}",
                    title: retry_label(&delivery.operation),
                    close_label: t!("delivery-cancel"),
                    busy: busy(),
                    on_close: move |_| {
                        if !busy() {
                            pending.set(None);
                            command_state.set(None);
                        }
                    },
                    p { {t!("admin-mail-delivery-confirm", recipient: delivery.recipient.clone())} }
                    dl { class: "admin-mail-detail__grid",
                        dt { {t!("admin-mail-delivery-id")} }
                        dd { TruncatedText { value: delivery.id.clone(), portal: true } }
                        dt { {t!("delivery-status")} }
                        dd { {delivery_status_label(&delivery.status)} }
                        dt { {t!("delivery-failure-code")} }
                        dd { TruncatedText { value: delivery.failure_code.clone().unwrap_or_else(|| t!("value-empty")), portal: true } }
                        dt { {t!("delivery-external-reference")} }
                        dd { TruncatedText { value: delivery.external_reference.clone().unwrap_or_else(|| t!("value-empty")), portal: true } }
                    }
                    if delivery.status == "manual_reconcile" {
                        p { class: "admin-mail-deliveries__warning", {t!("admin-mail-delivery-confirm-unknown")} }
                    }
                    if let Some(feedback) = dialog_error {
                        p { class: "action-feedback action-feedback--error", role: "alert", {feedback.message(String::new())} }
                    }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            r#type: "button",
                            autofocus: true,
                            disabled: busy(),
                            onclick: move |_| {
                                pending.set(None);
                                command_state.set(None);
                            },
                            {t!("delivery-cancel")}
                        }
                        button {
                            r#type: "button",
                            class: "admin-btn-primary",
                            disabled: busy(),
                            onclick: confirm_action,
                            if busy() { {t!("admin-action-working")} } else { {t!("delivery-confirm")} }
                        }
                    }
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
    let can_retry = retry_action(&delivery).is_some();
    let action = retry_label(&delivery.operation);
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
                if can_retry {
                    button {
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| on_retry.call(target.clone()),
                        "{action}"
                    }
                } else {
                    TruncatedText { value: t!("admin-mail-delivery-no-retry") }
                }
            }
        }
    }
}

fn retry_action(delivery: &AdminMailIntakeDelivery) -> Option<DeliveryAction> {
    if delivery.reason_code.as_deref() == Some("mail_disposition_conflict") {
        return None;
    }
    match delivery.status.as_str() {
        "failed" => Some(DeliveryAction::Retry),
        "manual_reconcile" => Some(DeliveryAction::ReconcileRetry),
        _ => None,
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
