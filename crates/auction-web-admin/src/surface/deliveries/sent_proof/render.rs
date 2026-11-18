//! 人工 Sent 证明对话框的渲染与展示文案。

use auction_web_shared::{Dialog, t};
use dioxus::prelude::*;

use super::evidence::{latest_send_attempt, selectable_evidence};
use super::preview::{preview_panel, run_preview, submit_proof};
use super::state::{
    SentProofDraft, SentProofIssue, SentProofPreviewError, SentProofReview, SentProofSubmission,
};
use super::validation::{missing_observed_ids, valid_reason};
use crate::api::DeliveryDetail;
use crate::surface::admin_world_label;

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
    #[props(default)] blocked: bool,
    on_submit: EventHandler<SentProofSubmission>,
    on_cancel: EventHandler<()>,
    on_stale: EventHandler<()>,
) -> Element {
    let candidates = selectable_evidence(&detail);
    let draft = use_signal(|| SentProofDraft::for_evidence(candidates.first().copied()));
    let mut review = use_signal(SentProofReview::default);
    use_effect(use_reactive((&detail,), move |_| review.write().clear()));
    #[cfg(test)]
    super::tests::capture_dialog_controls(draft, review);

    let current = draft();
    let selected =
        candidates.iter().copied().find(|item| item.reference == current.evidence_reference);
    let attempt = latest_send_attempt(&detail);
    let review_state = review();
    let local_issue = match review_state.outcome.as_ref() {
        Some(Err(SentProofPreviewError::Local(issue))) => Some(*issue),
        _ => None,
    };
    let fence = detail.delivery.fencing_version;
    let can_confirm = matches!(
        review_state.outcome.as_ref(),
        Some(Ok(proof)) if proof.can_confirm(&detail.delivery.id, fence)
    ) && valid_reason(&current.reason);

    let disabled = busy || blocked;
    let confirm_label =
        if busy { t!("admin-action-working") } else { t!("delivery-sent-proof-confirm") };
    let preview_label = if review_state.pending {
        t!("delivery-sent-proof-previewing")
    } else {
        t!("delivery-sent-proof-preview")
    };
    let preview_detail = detail.clone();
    let trigger_preview = move || run_preview(&preview_detail, disabled, draft, review, on_stale);
    let submit_delivery_id = detail.delivery.id.clone();

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
    let frozen_ids = attempt.map(|attempt| attempt.equipment_ids.as_slice());
    let observed_ids = selected.and_then(|item| item.remaining_equipment_ids());
    let before_collection = selected.and_then(|item| item.pre_collection_equipment_ids());
    let missing_ids = selected.zip(attempt).is_some_and(|(evidence, attempt)| {
        missing_observed_ids(evidence, attempt, current.collected)
    });
    let ids_match = frozen_ids.and_then(|frozen| {
        let observed_for_comparison = if current.collected && !frozen.is_empty() {
            before_collection.or_else(|| observed_ids.filter(|ids| !ids.is_empty()))
        } else {
            observed_ids
        };
        observed_for_comparison.map(|ids| {
            ids == frozen
                && before_collection.is_none_or(|ids| ids == frozen)
                && observed_ids.is_none_or(|ids| ids.is_empty() || ids == frozen)
        })
    });
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
            form { class: "admin-delivery-sent-proof", method: "post",
                aria_describedby: blocked.then_some("admin-sent-proof-occupied"),
                onsubmit: move |event| {
                    event.prevent_default();
                    if disabled {
                        return;
                    }
                    submit_proof(
                        &submit_delivery_id,
                        fence,
                        draft,
                        review,
                        on_submit,
                    );
                },
                p { class: "admin-panel__note", {t!("delivery-sent-proof-help")} }
                if blocked {
                    p { id: "admin-sent-proof-occupied", class: "admin-panel__note", {t!("delivery-command-occupied")} }
                }
                label { r#for: "delivery-sent-evidence", {t!("delivery-sent-proof-evidence")} }
                select {
                    id: "delivery-sent-evidence",
                    name: "evidence_reference",
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-evidence").to_string(),
                    aria_describedby: if missing_ids {
                        Some("delivery-sent-evidence-missing-ids".to_owned())
                    } else {
                        field_error_id(local_issue, "delivery-sent-evidence")
                    },
                    value: current.evidence_reference.clone(),
                    onchange: move |event| {
                        let next_reference = event.value();
                        let evidence = select_detail
                            .evidence
                            .iter()
                            .find(|item| item.reference == next_reference);
                        select_draft.set(select_draft().with_evidence(evidence));
                        review.write().clear();
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
                {field_error(local_issue, "delivery-sent-evidence")}
                if missing_ids {
                    p { id: "delivery-sent-evidence-missing-ids", class: "admin-panel__note",
                        {t!("delivery-sent-proof-missing-observed-ids")}
                    }
                }
                dl { class: "admin-delivery-sent-proof__grid",
                    id: "delivery-sent-evidence-summary",
                    tabindex: "-1",
                    aria_describedby: field_error_id(local_issue, "delivery-sent-evidence-summary"),
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
                {field_error(local_issue, "delivery-sent-evidence-summary")}
                div { class: "admin-delivery-sent-proof__grid",
                    dl {
                        dt { {t!("delivery-sent-proof-frozen-ids")} }
                        dd { {equipment_ids_text(frozen_ids)} }
                    }
                    dl {
                        dt { {t!("delivery-sent-proof-observed-ids")} }
                        dd { {equipment_ids_text(observed_ids)} }
                        if current.collected || before_collection.is_some() {
                            dt { {t!("delivery-sent-proof-pre-collection-ids")} }
                            dd { {equipment_ids_text(before_collection)} }
                        }
                    }
                }
                dl { class: "admin-delivery-sent-proof__grid",
                    dt { {t!("delivery-sent-proof-ids-comparison")} }
                    dd {
                        {match ids_match {
                            Some(true) => t!("delivery-sent-proof-ids-match"),
                            Some(false) => t!("delivery-sent-proof-ids-mismatch"),
                            None => t!("delivery-sent-proof-ids-unrecorded"),
                        }}
                    }
                }
                label { r#for: "delivery-sent-sender-name", {t!("delivery-sent-proof-sender-name")} }
                input {
                    id: "delivery-sent-sender-name",
                    name: "sender_name",
                    r#type: "text",
                    required: true,
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-sender-name").to_string(),
                    aria_describedby: field_error_id(local_issue, "delivery-sent-sender-name"),
                    value: current.sender_name.clone(),
                    oninput: move |event| {
                        let mut next = sender_name_draft();
                        next.sender_name = event.value();
                        sender_name_draft.set(next);
                        review.write().clear();
                    },
                }
                {field_error(local_issue, "delivery-sent-sender-name")}
                label { r#for: "delivery-sent-sender-uid", {t!("delivery-sent-proof-sender-uid")} }
                input {
                    id: "delivery-sent-sender-uid",
                    name: "sender_uid",
                    r#type: "text",
                    inputmode: "numeric",
                    required: true,
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-sender-uid").to_string(),
                    aria_describedby: field_error_id(local_issue, "delivery-sent-sender-uid"),
                    value: current.sender_uid.clone(),
                    oninput: move |event| {
                        let mut next = sender_uid_draft();
                        next.sender_uid = event.value();
                        sender_uid_draft.set(next);
                        review.write().clear();
                    },
                }
                {field_error(local_issue, "delivery-sent-sender-uid")}
                label { r#for: "delivery-sent-minute", {t!("delivery-sent-proof-sent-minute")} }
                input {
                    id: "delivery-sent-minute",
                    name: "sent_minute",
                    r#type: "datetime-local",
                    required: true,
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-minute").to_string(),
                    aria_describedby: field_error_id(local_issue, "delivery-sent-minute"),
                    value: current.sent_minute.clone(),
                    oninput: move |event| {
                        let mut next = minute_draft();
                        let value = event.value();
                        if next.sent_minute == value {
                            return;
                        }
                        next.sent_minute = value;
                        next.sent_minute_edited = true;
                        minute_draft.set(next);
                        review.write().clear();
                    },
                }
                {field_error(local_issue, "delivery-sent-minute")}
                label { r#for: "delivery-sent-remaining-credits", {t!("delivery-sent-proof-remaining-credits")} }
                input {
                    id: "delivery-sent-remaining-credits",
                    name: "remaining_credits",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-remaining-credits").to_string(),
                    aria_describedby: field_error_id(local_issue, "delivery-sent-remaining-credits"),
                    value: current.remaining_credits.clone(),
                    oninput: move |event| {
                        let mut next = credits_draft();
                        next.remaining_credits = event.value();
                        credits_draft.set(next);
                        review.write().clear();
                    },
                }
                {field_error(local_issue, "delivery-sent-remaining-credits")}
                label { r#for: "delivery-sent-cod-pending", {t!("delivery-sent-proof-cod-pending")} }
                input {
                    id: "delivery-sent-cod-pending",
                    name: "cod_pending",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: disabled,
                    aria_invalid: field_is_invalid(local_issue, "delivery-sent-cod-pending").to_string(),
                    aria_describedby: field_error_id(local_issue, "delivery-sent-cod-pending"),
                    value: current.cod_pending.clone(),
                    oninput: move |event| {
                        let mut next = cod_draft();
                        next.cod_pending = event.value();
                        cod_draft.set(next);
                        review.write().clear();
                    },
                }
                {field_error(local_issue, "delivery-sent-cod-pending")}
                label { class: "admin-delivery-sent-proof__check", r#for: "delivery-sent-collected",
                    input {
                        id: "delivery-sent-collected",
                        r#type: "checkbox",
                        name: "collected",
                        disabled: disabled,
                        checked: current.collected,
                        onchange: move |event| {
                            let mut next = collected_draft();
                            next.collected = event.checked();
                            collected_draft.set(next);
                            review.write().clear();
                        },
                    }
                    {t!("delivery-sent-proof-collected")}
                }
                if current.collected {
                    label { r#for: "delivery-sent-receipt", {t!("delivery-sent-proof-receipt")} }
                    textarea {
                        id: "delivery-sent-receipt",
                        name: "receipt",
                        rows: "3",
                        disabled: disabled,
                        aria_invalid: field_is_invalid(local_issue, "delivery-sent-receipt").to_string(),
                        aria_describedby: field_error_id(local_issue, "delivery-sent-receipt"),
                        value: current.receipt.clone(),
                        oninput: move |event| {
                            let mut next = receipt_draft();
                            next.receipt = event.value();
                            receipt_draft.set(next);
                            review.write().clear();
                        },
                    }
                    {field_error(local_issue, "delivery-sent-receipt")}
                }
                label { r#for: "delivery-sent-reason", {t!("delivery-sent-proof-reason")} }
                textarea {
                    id: "delivery-sent-reason",
                    name: "reason",
                    rows: "3",
                    required: true,
                    disabled: disabled,
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
                        disabled: disabled || review_state.pending || missing_ids,
                        onclick: move |_| trigger_preview(),
                        "{preview_label}"
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
                        disabled: disabled || !can_confirm,
                        "{confirm_label}"
                    }
                }
                {preview_panel(&review_state, disabled, review)}
            }
        }
    }
}

fn equipment_ids_text(ids: Option<&[String]>) -> String {
    match ids {
        None => t!("delivery-sent-proof-ids-unrecorded"),
        Some([]) => t!("delivery-sent-proof-ids-empty"),
        Some(ids) => ids.join(", "),
    }
}

fn field_is_invalid(issue: Option<SentProofIssue>, field: &str) -> bool {
    issue.is_some_and(|issue| issue.field_id() == field)
}

fn field_error_id(issue: Option<SentProofIssue>, field: &str) -> Option<String> {
    field_is_invalid(issue, field).then(|| format!("{field}-error"))
}

fn field_error(issue: Option<SentProofIssue>, field: &str) -> Element {
    rsx! {
        if let Some(issue) = issue.filter(|issue| issue.field_id() == field) {
            p { id: "{field}-error", class: "action-feedback action-feedback--error",
                {issue.message()}
            }
        }
    }
}
