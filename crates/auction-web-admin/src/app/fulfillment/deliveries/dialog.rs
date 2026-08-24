//! Delivery release/retry/reconcile 确认对话框。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;

use crate::api::{Delivery, DeliveryAction};

use super::helpers::delivery_status_label;
use super::model::{DeliveryActionRequest, DeliveryDialogKind};

/// 展示 delivery 动作的确认和人工对账字段。
#[component]
pub(super) fn DeliveryDialog(
    delivery: Delivery,
    kind: DeliveryDialogKind,
    busy: bool,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut reconcile_action = use_signal(|| "retry".to_owned());
    let mut external_reference = use_signal(String::new);
    let mut failure_code = use_signal(String::new);
    let title = kind.title();
    let delivery_id = delivery.id.clone();
    let expected_fencing_version = delivery.fencing_version;
    let status = delivery_status_label(&delivery.status);
    let failure = delivery.failure_code.clone().unwrap_or_else(|| t!("value-empty"));

    rsx! {
        div { class: "admin-delivery__dialog-backdrop", role: "presentation",
            section {
                class: "admin-delivery__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "delivery-dialog-title",
                aria_busy: busy.to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy {
                        on_cancel.call(());
                    }
                },
                h2 { id: "delivery-dialog-title", {title} }
                p { class: "admin-panel__note", "{delivery.recipient} · {status} · {failure}" }
                if kind == DeliveryDialogKind::Reconcile {
                    form {
                        onsubmit: move |event| {
                            event.prevent_default();
                            if busy {
                                return;
                            }
                            let selected_action = reconcile_action();
                            let external = external_reference().trim().to_owned();
                            let failure = failure_code().trim().to_owned();
                            if selected_action == "succeeded" && external.is_empty()
                                || selected_action == "failed" && failure.is_empty()
                            {
                                return;
                            }
                            let action = match selected_action.as_str() {
                                "succeeded" => DeliveryAction::ReconcileSucceeded,
                                "failed" => DeliveryAction::ReconcileFailed,
                                _ => DeliveryAction::ReconcileRetry,
                            };
                            on_confirm.call(DeliveryActionRequest {
                                delivery_id: delivery_id.clone(),
                                action,
                                expected_fencing_version,
                                external_reference: (action == DeliveryAction::ReconcileSucceeded)
                                    .then_some(external),
                                failure_code: (action == DeliveryAction::ReconcileFailed)
                                    .then_some(failure),
                            });
                        },
                        label {
                            {t!("delivery-reconcile-action")}
                            select {
                                value: reconcile_action(),
                                disabled: busy,
                                onchange: move |event| reconcile_action.set(event.value()),
                                option { value: "retry", {t!("delivery-reconcile-action-retry")} }
                                option { value: "succeeded", {t!("delivery-reconcile-action-succeeded")} }
                                option { value: "failed", {t!("delivery-reconcile-action-failed")} }
                            }
                        }
                        if reconcile_action() == "succeeded" {
                            label {
                                {t!("delivery-external-reference")}
                                input {
                                    r#type: "text",
                                    maxlength: "256",
                                    required: true,
                                    disabled: busy,
                                    value: external_reference(),
                                    oninput: move |event| external_reference.set(event.value()),
                                }
                            }
                        }
                        if reconcile_action() == "failed" {
                            label {
                                {t!("delivery-failure-code")}
                                input {
                                    r#type: "text",
                                    maxlength: "128",
                                    required: true,
                                    disabled: busy,
                                    value: failure_code(),
                                    oninput: move |event| failure_code.set(event.value()),
                                }
                            }
                        }
                        DeliveryDialogActions {
                            busy,
                            confirm_label: t!("delivery-reconcile"),
                            on_cancel,
                        }
                    }
                } else {
                    div { class: "admin-delivery__dialog-actions",
                        button {
                            r#type: "button",
                            class: "admin-btn-secondary",
                            disabled: busy,
                            autofocus: true,
                            onclick: move |_| on_cancel.call(()),
                            {t!("delivery-cancel")}
                        }
                        button {
                            r#type: "button",
                            class: "admin-btn-primary",
                            disabled: busy,
                            onclick: move |_| on_confirm.call(DeliveryActionRequest {
                                delivery_id: delivery_id.clone(),
                                action: kind.action(),
                                expected_fencing_version,
                                external_reference: None,
                                failure_code: None,
                            }),
                            if busy { {t!("admin-action-working")} } else { {t!("delivery-confirm")} }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn DeliveryDialogActions(
    busy: bool,
    confirm_label: String,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-delivery__dialog-actions",
            button {
                r#type: "button",
                class: "admin-btn-secondary",
                disabled: busy,
                autofocus: true,
                onclick: move |_| on_cancel.call(()),
                {t!("delivery-cancel")}
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy,
                if busy { {t!("admin-action-working")} } else { {confirm_label} }
            }
        }
    }
}
