//! 人工 Sent 证明对话框的渲染与展示文案。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use crate::api::{DeliveryDetail, DeliverySentProofPreview};
use crate::surface::admin_world_label;

use super::evidence::{latest_send_attempt, selectable_evidence};
use super::preview::{preview_panel, run_preview, submit_proof};
use super::state::{SentProofDraft, SentProofSubmission};
use super::validation::valid_reason;

/// 证据种类译文；未知种类原样显示。
pub(crate) fn evidence_kind_label(kind: &str) -> String {
    match kind {
        "mailbox_sweep" => t!("delivery-detail-evidence-kind-mailbox-sweep"),
        "sent_reference_sighting" => t!("delivery-detail-evidence-kind-sent-sighting"),
        "compose_discard" => t!("delivery-detail-evidence-kind-compose-discard"),
        "sent_operator_confirmed" => t!("delivery-detail-evidence-kind-operator-confirmed"),
        other => other.to_owned(),
    }
}

/// 展示可以从详情证据中选用的人工证明对话框。
#[component]
pub(crate) fn SentProofDialog(
    detail: DeliveryDetail,
    busy: bool,
    on_submit: EventHandler<SentProofSubmission>,
    on_cancel: EventHandler<()>,
) -> Element {
    let candidates = selectable_evidence(&detail);
    let draft = use_signal(|| SentProofDraft::for_evidence(candidates.first().copied()));
    let mut preview = use_signal(|| None::<Result<DeliverySentProofPreview, String>>);
    let previewing = use_signal(|| false);
    let accept_collected = use_signal(|| false);
    let accept_attempts = use_signal(|| false);

    let current = draft();
    let selected =
        candidates.iter().copied().find(|item| item.reference == current.evidence_reference);
    let attempt = latest_send_attempt(&detail);
    let preview_state = preview();
    let requires_collected = matches!(
        preview_state.as_ref(),
        Some(Ok(preview)) if preview.requires_collected_asset_acceptance
    );
    let requires_attempts = matches!(
        preview_state.as_ref(),
        Some(Ok(preview)) if preview.requires_attempt_history_acceptance
    );
    let eligible = matches!(preview_state.as_ref(), Some(Ok(preview)) if preview.eligible);
    let can_confirm = !busy
        && eligible
        && (!requires_collected || accept_collected())
        && (!requires_attempts || accept_attempts())
        && valid_reason(&current.reason);

    let preview_detail = detail.clone();
    let preview_draft = draft;
    let trigger_preview =
        move || run_preview(&preview_detail, busy, preview_draft, preview, previewing);

    let submit_detail = detail.clone();
    let submit_draft = draft;
    let submit_accept_collected = accept_collected;
    let submit_accept_attempts = accept_attempts;

    let world = attempt.map(|attempt| admin_world_label(&attempt.world_id));
    let mid = selected.and_then(|item| item.external_mail_id.clone());
    let recipient = selected
        .and_then(|item| item.recipient.clone())
        .or_else(|| attempt.and_then(|attempt| attempt.recipient.clone()));
    let subject = selected
        .and_then(|item| item.subject.clone())
        .or_else(|| attempt.and_then(|attempt| attempt.subject.clone()));
    let original_digest = attempt.and_then(|attempt| attempt.body_digest.clone());
    let observed_digest = selected.and_then(|item| item.body_digest.clone());
    let select_detail = detail.clone();
    let mut select_draft = draft;
    let mut sender_name_draft = draft;
    let mut sender_uid_draft = draft;
    let mut minute_draft = draft;
    let mut credits_draft = draft;
    let mut cod_draft = draft;
    let mut collected_draft = draft;
    let mut receipt_draft = draft;
    let mut reason_draft = draft;

    rsx! {
        Dialog {
            title: t!("delivery-sent-proof-title"),
            close_label: t!("delivery-cancel"),
            busy,
            on_close: move |_| on_cancel.call(()),
            form { class: "admin-delivery-sent-proof",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy {
                        return;
                    }
                    submit_proof(
                        &submit_detail,
                        submit_draft,
                        submit_accept_collected,
                        submit_accept_attempts,
                        can_confirm,
                        on_submit,
                    );
                },
                p { class: "admin-panel__note", {t!("delivery-sent-proof-help")} }
                label { r#for: "delivery-sent-evidence", {t!("delivery-sent-proof-evidence")} }
                select {
                    id: "delivery-sent-evidence",
                    disabled: busy,
                    value: current.evidence_reference.clone(),
                    onchange: move |event| {
                        let next_reference = event.value();
                        let evidence = select_detail
                            .evidence
                            .iter()
                            .find(|item| item.reference == next_reference);
                        select_draft.set(select_draft().with_evidence(evidence));
                        preview.set(None);
                    },
                    option {
                        value: "",
                        disabled: true,
                        selected: current.evidence_reference.is_empty(),
                        {t!("delivery-sent-proof-evidence-placeholder")}
                    }
                    for item in candidates.iter() {
                        option {
                            key: "{item.reference}",
                            value: item.reference.clone(),
                            selected: current.evidence_reference == item.reference,
                            "{evidence_kind_label(&item.kind)} · {item.external_mail_id.clone().unwrap_or_default()}"
                        }
                    }
                }
                dl { class: "admin-delivery-sent-proof__grid",
                    dt { {t!("delivery-sent-proof-world")} }
                    dd { "{world.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-mid")} }
                    dd { "{mid.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-recipient")} }
                    dd { "{recipient.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-subject")} }
                    dd { "{subject.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-original-digest")} }
                    dd {
                        code { "{original_digest.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    }
                    dt { {t!("delivery-sent-proof-observed-digest")} }
                    dd {
                        code { "{observed_digest.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    }
                }
                label { r#for: "delivery-sent-sender-name", {t!("delivery-sent-proof-sender-name")} }
                input {
                    id: "delivery-sent-sender-name",
                    r#type: "text",
                    required: true,
                    disabled: busy,
                    value: current.sender_name.clone(),
                    oninput: move |event| {
                        let mut next = sender_name_draft();
                        next.sender_name = event.value();
                        sender_name_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-sender-uid", {t!("delivery-sent-proof-sender-uid")} }
                input {
                    id: "delivery-sent-sender-uid",
                    r#type: "text",
                    inputmode: "numeric",
                    required: true,
                    disabled: busy,
                    value: current.sender_uid.clone(),
                    oninput: move |event| {
                        let mut next = sender_uid_draft();
                        next.sender_uid = event.value();
                        sender_uid_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-minute", {t!("delivery-sent-proof-sent-minute")} }
                input {
                    id: "delivery-sent-minute",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy,
                    value: current.sent_minute.clone(),
                    oninput: move |event| {
                        let mut next = minute_draft();
                        next.sent_minute = event.value();
                        minute_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-remaining-credits", {t!("delivery-sent-proof-remaining-credits")} }
                input {
                    id: "delivery-sent-remaining-credits",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: busy,
                    value: current.remaining_credits.clone(),
                    oninput: move |event| {
                        let mut next = credits_draft();
                        next.remaining_credits = event.value();
                        credits_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-cod-pending", {t!("delivery-sent-proof-cod-pending")} }
                input {
                    id: "delivery-sent-cod-pending",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: busy,
                    value: current.cod_pending.clone(),
                    oninput: move |event| {
                        let mut next = cod_draft();
                        next.cod_pending = event.value();
                        cod_draft.set(next);
                        preview.set(None);
                    },
                }
                label { class: "admin-delivery-sent-proof__check",
                    input {
                        r#type: "checkbox",
                        disabled: busy,
                        checked: current.collected,
                        onchange: move |event| {
                            let mut next = collected_draft();
                            next.collected = event.checked();
                            collected_draft.set(next);
                            preview.set(None);
                        },
                    }
                    {t!("delivery-sent-proof-collected")}
                }
                if current.collected {
                    label { r#for: "delivery-sent-receipt", {t!("delivery-sent-proof-receipt")} }
                    textarea {
                        id: "delivery-sent-receipt",
                        rows: "3",
                        disabled: busy,
                        value: current.receipt.clone(),
                        oninput: move |event| {
                            let mut next = receipt_draft();
                            next.receipt = event.value();
                            receipt_draft.set(next);
                            preview.set(None);
                        },
                    }
                }
                label { r#for: "delivery-sent-reason", {t!("delivery-sent-proof-reason")} }
                textarea {
                    id: "delivery-sent-reason",
                    rows: "3",
                    required: true,
                    disabled: busy,
                    value: current.reason.clone(),
                    oninput: move |event| {
                        let mut next = reason_draft();
                        next.reason = event.value();
                        reason_draft.set(next);
                    },
                }
                p { class: "admin-panel__note", {t!("delivery-sent-proof-note")} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        disabled: busy || previewing(),
                        onclick: move |_| trigger_preview(),
                        if previewing() {
                            {t!("delivery-sent-proof-previewing")}
                        } else {
                            {t!("delivery-sent-proof-preview")}
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| on_cancel.call(()),
                        {t!("delivery-cancel")}
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: busy || !can_confirm,
                        if busy {
                            {t!("admin-action-working")}
                        } else {
                            {t!("delivery-sent-proof-confirm")}
                        }
                    }
                }
                if let Some(result) = preview_state {
                    {preview_panel(result, busy, accept_collected, accept_attempts)}
                }
            }
        }
    }
}
