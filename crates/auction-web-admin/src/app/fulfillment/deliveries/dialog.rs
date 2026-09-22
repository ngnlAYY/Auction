//! Delivery release/retry/reconcile 确认对话框。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{Delivery, DeliveryAction};

use super::helpers::{delivery_display_id, is_mail_disposition_conflict};
use super::model::{DeliveryActionRequest, DeliveryDialogKind, DeliveryReconcileDraft};

/// 展示 delivery 动作的确认和人工对账字段。
#[component]
pub(super) fn DeliveryDialog(
    delivery: Delivery,
    kind: DeliveryDialogKind,
    busy: bool,
    draft: DeliveryReconcileDraft,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let display_id = delivery_display_id(&delivery);
    let disposition_conflict = is_mail_disposition_conflict(&delivery);
    let title = match kind {
        DeliveryDialogKind::Release => t!("delivery-release"),
        DeliveryDialogKind::Retry | DeliveryDialogKind::ReconcileRetry => t!("delivery-retry"),
        DeliveryDialogKind::Reconcile => t!("delivery-reconcile-title", id: display_id),
    };
    let delivery_id = delivery.id.clone();
    let reconcile_delivery_id = delivery_id.clone();
    let expected_fencing_version = delivery.fencing_version;
    let draft_delivery_id = draft.delivery_id.clone();
    let draft_action = draft.action.clone();
    let draft_external_reference = draft.external_reference.clone();
    let draft_failure_code = draft.failure_code.clone();
    let submit_action = draft_action.clone();
    let submit_external_reference = draft_external_reference.clone();
    let submit_failure_code = draft_failure_code.clone();
    let select_delivery_id = draft_delivery_id.clone();
    let select_external_reference = draft_external_reference.clone();
    let select_failure_code = draft_failure_code.clone();
    let external_delivery_id = draft_delivery_id.clone();
    let external_action = draft_action.clone();
    let external_failure_code = draft_failure_code.clone();
    let failure_delivery_id = draft_delivery_id.clone();
    let failure_action = draft_action.clone();
    let failure_external_reference = draft_external_reference.clone();
    let failure = delivery.failure_code.clone().unwrap_or_else(|| "failed".to_owned());
    let is_send_delivery = delivery.cod.is_some();
    let send_success_blocked = is_send_delivery && draft_action == "succeeded";
    let dialog_identity = match kind {
        DeliveryDialogKind::Release => "release",
        DeliveryDialogKind::Retry => "retry",
        DeliveryDialogKind::Reconcile => "reconcile",
        DeliveryDialogKind::ReconcileRetry => "reconcile-retry",
    };

    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；key 保留以区分不同 delivery
    // 动作的弹窗实例。
    rsx! {
        Dialog {
            key: "{dialog_identity}",
            title,
            close_label: t!("delivery-cancel"),
            busy,
            on_close: move |_| on_cancel.call(()),
            if kind == DeliveryDialogKind::Reconcile {
                    form {
                        class: "admin-delivery-reconcile",
                        onsubmit: move |event| {
                            event.prevent_default();
                            if busy || send_success_blocked
                                || (disposition_conflict && submit_action != "failed") {
                                return;
                            }
                            let external = submit_external_reference.trim().to_owned();
                            let failure = submit_failure_code.trim().to_owned();
                            let action = match submit_action.as_str() {
                                "succeeded" => DeliveryAction::ReconcileSucceeded,
                                "failed" => DeliveryAction::ReconcileFailed,
                                _ => DeliveryAction::ReconcileRetry,
                            };
                            on_confirm.call(DeliveryActionRequest {
                                delivery_id: reconcile_delivery_id.clone(),
                                action,
                                expected_fencing_version,
                                external_reference: (action == DeliveryAction::ReconcileSucceeded)
                                    .then_some(external),
                                failure_code: (action == DeliveryAction::ReconcileFailed)
                                    .then_some(failure),
                            });
                        },
                        label { r#for: "delivery-reconcile-action-{delivery_id}", {t!("delivery-reconcile-action")} }
                        if disposition_conflict {
                            p { class: "admin-delivery-reconcile__warning", {t!("delivery-mail-disposition-conflict")} }
                        }
                        if send_success_blocked {
                            p { class: "admin-delivery-reconcile__warning", {t!("delivery-reconcile-send-proof-required")} }
                            button {
                                r#type: "button",
                                disabled: busy,
                                onclick: move |_| on_cancel.call(()),
                                {t!("delivery-cancel")}
                            }
                        } else {
                            select {
                                id: "delivery-reconcile-action-{delivery_id}",
                                value: draft_action.clone(),
                                onchange: move |event| {
                                    on_draft_change.call(DeliveryReconcileDraft {
                                        delivery_id: select_delivery_id.clone(),
                                        action: event.value(),
                                        external_reference: select_external_reference.clone(),
                                        failure_code: select_failure_code.clone(),
                                    });
                                },
                                if !disposition_conflict {
                                    option { value: "retry", {t!("delivery-reconcile-action-retry")} }
                                }
                                if !is_send_delivery && !disposition_conflict {
                                    option { value: "succeeded", {t!("delivery-reconcile-action-succeeded")} }
                                }
                                option { value: "failed", {t!("delivery-reconcile-action-failed")} }
                            }
                        }
                        if draft_action == "succeeded" && !send_success_blocked {
                            label { r#for: "delivery-external-reference-{delivery_id}", {t!("delivery-external-reference")} }
                            input {
                                id: "delivery-external-reference-{delivery_id}",
                                r#type: "text",
                                required: true,
                                value: draft_external_reference.clone(),
                                oninput: move |event| {
                                    on_draft_change.call(DeliveryReconcileDraft {
                                        delivery_id: external_delivery_id.clone(),
                                        action: external_action.clone(),
                                        external_reference: event.value(),
                                        failure_code: external_failure_code.clone(),
                                    });
                                },
                            }
                        }
                        if !send_success_blocked {
                            if draft_action == "failed" {
                                label { r#for: "delivery-failure-code-{delivery_id}", {t!("delivery-failure-code")} }
                                input {
                                    id: "delivery-failure-code-{delivery_id}",
                                    r#type: "text",
                                    required: true,
                                    value: draft_failure_code.clone(),
                                    oninput: move |event| {
                                        on_draft_change.call(DeliveryReconcileDraft {
                                            delivery_id: failure_delivery_id.clone(),
                                            action: failure_action.clone(),
                                            external_reference: failure_external_reference.clone(),
                                            failure_code: event.value(),
                                        });
                                    },
                                }
                            }
                            DeliveryDialogActions {
                                busy,
                                confirm_label: t!("delivery-reconcile"),
                                on_cancel,
                            }
                        }
                    }
                } else {
                    p { "{delivery.recipient} · {failure}" }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            r#type: "button",
                            disabled: busy,
                            onclick: move |_| on_confirm.call(DeliveryActionRequest {
                                delivery_id: delivery_id.clone(),
                                action: kind.action(),
                                expected_fencing_version,
                                external_reference: None,
                                failure_code: None,
                            }),
                            {t!("delivery-confirm")}
                        }
                        button {
                            r#type: "button",
                            disabled: busy,
                            autofocus: true,
                            onclick: move |_| on_cancel.call(()),
                            {t!("delivery-cancel")}
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
        div { class: "admin-shell__dialog-actions",
            button {
                r#type: "button",
                disabled: busy,
                autofocus: true,
                onclick: move |_| on_cancel.call(()),
                {t!("delivery-cancel")}
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy,
                "{confirm_label}"
            }
        }
    }
}
