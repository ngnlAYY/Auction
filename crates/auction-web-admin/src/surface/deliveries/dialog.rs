//! Delivery release/retry/reconcile 确认对话框。

use auction_contracts::ADMIN_DELIVERY_RECONCILE_FAILURE_CODES;
use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::helpers::{delivery_display_id, is_mail_disposition_conflict};
use super::model::{DeliveryActionRequest, DeliveryDialogKind, DeliveryReconcileDraft};
use super::sent_proof::evidence_kind_label;
use crate::api::{Delivery, DeliveryAction, fetch_delivery_detail};
use crate::surface::format_admin_time;

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
    let draft_failure_code = draft.failure_code;
    let submit_action = draft_action.clone();
    let submit_external_reference = draft_external_reference.clone();
    let submit_failure_code = draft_failure_code.clone();
    let select_delivery_id = draft_delivery_id.clone();
    let select_external_reference = draft_external_reference.clone();
    let select_failure_code = draft_failure_code.clone();
    let external_delivery_id = draft_delivery_id.clone();
    let external_action = draft_action.clone();
    let external_failure_code = draft_failure_code.clone();
    let failure_delivery_id = draft_delivery_id;
    let failure_action = draft_action.clone();
    let failure_external_reference = draft_external_reference.clone();
    let failure = delivery.failure_code.clone().unwrap_or_else(|| "failed".to_owned());
    let is_send_delivery = delivery.cod.is_some();
    let send_success_blocked = is_send_delivery && draft_action == "succeeded";

    // 外壳只拥有焦点和标题 ID；草稿与已提交命令由页面持有，不靠内层 key 重建。
    rsx! {
        Dialog {
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
                        ReconcileEvidenceCandidates { delivery_id: delivery_id.clone() }
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
                            // 条件选项可能晚于 select.value 挂载；逐项 selected 保证显示与提交动作一致。
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
                                    option { value: "retry", selected: draft_action == "retry", {t!("delivery-reconcile-action-retry")} }
                                }
                                if !is_send_delivery && !disposition_conflict {
                                    option { value: "succeeded", selected: draft_action == "succeeded", {t!("delivery-reconcile-action-succeeded")} }
                                }
                                option { value: "failed", selected: draft_action == "failed", {t!("delivery-reconcile-action-failed")} }
                            }
                        }
                        if draft_action == "succeeded" && !send_success_blocked {
                            label { r#for: "delivery-external-reference-{delivery_id}", {t!("delivery-external-reference")} }
                            input {
                                id: "delivery-external-reference-{delivery_id}",
                                r#type: "text",
                                required: true,
                                value: draft_external_reference,
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
                                // 判失败只提交后端闭集原因码；空占位使未选择时不能提交。
                                select {
                                    id: "delivery-failure-code-{delivery_id}",
                                    required: true,
                                    value: draft_failure_code.clone(),
                                    onchange: move |event| {
                                        on_draft_change.call(DeliveryReconcileDraft {
                                            delivery_id: failure_delivery_id.clone(),
                                            action: failure_action.clone(),
                                            external_reference: failure_external_reference.clone(),
                                            failure_code: event.value(),
                                        });
                                    },
                                    option {
                                        value: "",
                                        disabled: true,
                                        selected: draft_failure_code.is_empty(),
                                        {t!("delivery-failure-code-placeholder")}
                                    }
                                    for code in ADMIN_DELIVERY_RECONCILE_FAILURE_CODES {
                                        option {
                                            key: "{code}",
                                            value: code,
                                            selected: draft_failure_code == code,
                                            {failure_code_label(code)}
                                        }
                                    }
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

/// 人工对账前展示该投递已写入的 Sent 目击与候选；只读，不改变判失败闭集或命令语义。
/// 维护证据是结果未知时的首要线索：重试或判失败前应先核对 MID、时间与领取状态。
#[component]
fn ReconcileEvidenceCandidates(delivery_id: String) -> Element {
    let detail = use_resource(use_reactive((&delivery_id,), move |(delivery_id,)| async move {
        let result = fetch_delivery_detail(delivery_id.clone()).await;
        (delivery_id, result)
    }));
    let loading = detail.state().cloned() != UseResourceState::Ready;
    let loaded = if loading {
        None
    } else {
        detail
            .read()
            .as_ref()
            .filter(|(loaded_id, _)| loaded_id == &delivery_id)
            .map(|(_, result)| result.clone())
    };
    let Some(Ok(detail)) = loaded else {
        return rsx! {};
    };
    rsx! {
        section { class: "admin-delivery-reconcile__evidence",
            h3 { {t!("delivery-reconcile-sightings")} }
            p { class: "admin-panel__note", {t!("delivery-reconcile-sightings-help")} }
            if detail.evidence.is_empty() {
                p { class: "admin-panel__note", {t!("delivery-reconcile-sightings-empty")} }
            } else {
                ul { class: "admin-delivery-reconcile__sightings",
                    for item in detail.evidence {
                        li { key: "{item.reference}",
                            span { class: "admin-delivery-reconcile__kind", {evidence_kind_label(&item.kind)} }
                            code { {item.external_mail_id.clone().unwrap_or_else(|| t!("value-empty"))} }
                            span { {observed_label(&item.observed_at)} }
                            span { {collected_label(item.collected)} }
                        }
                    }
                }
            }
        }
    }
}

fn observed_label(value: &Option<String>) -> String {
    value.as_deref().map_or_else(|| t!("value-empty"), format_admin_time)
}

fn collected_label(value: Option<bool>) -> String {
    match value {
        Some(true) => t!("delivery-detail-yes"),
        Some(false) => t!("delivery-detail-no"),
        None => t!("delivery-detail-custody-unobserved"),
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

/// 闭集失败原因码的界面译文；未知码原样显示，不猜测语义。
fn failure_code_label(code: &str) -> String {
    match code {
        "verified_not_sent" => t!("delivery-failure-code-verified-not-sent"),
        "recipient_invalid" => t!("delivery-failure-code-recipient-invalid"),
        "no_longer_needed" => t!("delivery-failure-code-no-longer-needed"),
        other => other.to_owned(),
    }
}
