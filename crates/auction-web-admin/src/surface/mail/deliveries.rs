//! 收件关联投递的安全展示与手动重排；原邮件处理结果和外部完成证明仍由后端拥有。

use auction_web_shared::{Dialog, TableColumns, TruncatedText, t};
use dioxus::prelude::*;

use super::super::deliveries::labels::delivery_status_label;
use crate::api::{AdminMailIntakeDelivery, DeliveryAction, execute_delivery};
use crate::surface::AdminActionFeedback;
use crate::surface::pagination::{AdminPager as Pager, use_admin_pagination};
use crate::surface::pending::{PendingCommands, set_if_alive};

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
        if commands.contains(&delivery.id) {
            return;
        }
        let success_intake_id = confirm_intake_id.clone();
        let failure_intake_id = confirm_intake_id.clone();
        notice.set(None);
        let submitted = commands.submit(
            delivery.id.clone(),
            retry_label(&delivery.operation),
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
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
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
            pending.set(None);
        }
    };
    let total = i64::try_from(deliveries.len()).unwrap_or(i64::MAX);
    let current_page = page().clamp(1, if total == 0 { 1 } else { 1 + (total - 1) / page_size });
    let offset =
        usize::try_from((current_page - 1).saturating_mul(page_size)).unwrap_or(usize::MAX);
    let visible =
        deliveries.into_iter().skip(offset).take(usize::try_from(page_size).unwrap_or(usize::MAX));
    let dialog_error = notice().and_then(|(source, feedback)| {
        (source == intake_id && !feedback.is_success()).then_some(feedback)
    });

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
                Dialog {
                    title: retry_label(&delivery.operation),
                    close_label: t!("delivery-cancel"),
                    busy: busy(),
                    on_close: move |_| {
                        if !busy() {
                            pending.set(None);
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
