//! 投递详情的只读分区：摘要、冻结尝试、Sent 证据、托管、处置与来源义务。

use auction_web_shared::{TableColumns, TruncatedText, t};
use dioxus::prelude::*;

use super::super::labels::{delivery_display_type, delivery_status_label};
use super::super::sent_proof::evidence_kind_label;
use crate::api::{
    DeliveryAttempt, DeliveryCustody, DeliveryDetail, DeliveryDisposition, DeliveryEvidence,
    DeliveryObligation, DeliveryRecovery,
};
use crate::surface::{admin_world_label, format_admin_time};

#[component]
pub(super) fn DeliverySummary(detail: DeliveryDetail) -> Element {
    let delivery = detail.delivery;
    let value = |value: Option<String>| value.unwrap_or_else(|| t!("value-empty"));
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-summary-title",
            h2 { id: "delivery-detail-summary-title", {t!("delivery-detail-summary")} }
            dl { class: "admin-delivery-detail__grid",
                dt { {t!("delivery-id")} }
                dd { TruncatedText { value: delivery.id.clone(), portal: true } }
                dt { {t!("delivery-type")} }
                dd { "{delivery_display_type(&delivery.origin_kind)}" }
                dt { {t!("delivery-origin-id")} }
                dd { TruncatedText { value: delivery.origin_id.clone(), portal: true } }
                dt { {t!("delivery-to")} }
                dd { "{delivery.recipient}" }
                dt { {t!("delivery-cod")} }
                dd {
                    "{delivery.cod.map_or_else(|| t!(\"value-empty\"), |cod| cod.to_string())}"
                }
                dt { {t!("delivery-status")} }
                dd { span { class: "admin-delivery-badge", {delivery_status_label(&delivery.status)} } }
                dt { {t!("delivery-detail-fencing")} }
                dd { "{delivery.fencing_version}" }
                dt { {t!("delivery-attempts")} }
                dd { "{delivery.attempt_count}" }
                dt { {t!("delivery-detail-created")} }
                dd { "{format_admin_time(&delivery.created_at)}" }
                dt { {t!("delivery-detail-updated")} }
                dd { "{format_admin_time(&delivery.updated_at)}" }
                dt { {t!("delivery-detail-claimed")} }
                dd { "{format_admin_time(&value(delivery.claimed_at.clone()))}" }
                dt { {t!("delivery-detail-sending")} }
                dd { "{format_admin_time(&value(delivery.sending_at.clone()))}" }
                dt { {t!("delivery-detail-completed")} }
                dd { "{format_admin_time(&value(delivery.completed_at.clone()))}" }
                dt { {t!("delivery-external-reference")} }
                dd { TruncatedText { value: value(delivery.external_reference.clone()), portal: true } }
                dt { {t!("delivery-failure-code")} }
                dd { "{value(delivery.failure_code.clone())}" }
                dt { {t!("delivery-detail-reason-code")} }
                dd { "{value(delivery.reason_code.clone())}" }
                dt { {t!("delivery-detail-returned-mid")} }
                dd { "{value(delivery.returned_mail_id.clone())}" }
                dt { {t!("delivery-detail-take-required")} }
                dd {
                    if delivery.take_required {
                        {t!("delivery-detail-yes")}
                    } else {
                        {t!("delivery-detail-no")}
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AttemptSection(attempts: Vec<DeliveryAttempt>) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-attempts-title",
            h2 { id: "delivery-detail-attempts-title", {t!("delivery-detail-attempts")} }
            if attempts.is_empty() {
                p { class: "admin-panel__note", {t!("delivery-detail-attempts-empty")} }
            } else {
                div { class: "admin-shell__table-wrap",
                    table { class: "admin-shell__table admin-delivery-detail__table",
                        aria_label: t!("delivery-detail-attempts"),
                        TableColumns { layout: "admin-delivery-attempts", widths: vec![9, 12, 9, 8, 12, 16, 12, 10, 12] }
                        thead {
                            tr {
                                th { scope: "col", {t!("delivery-detail-attempt-operation")} }
                                th { scope: "col", {t!("delivery-detail-attempt-time")} }
                                th { scope: "col", {t!("delivery-detail-attempt-fence")} }
                                th { scope: "col", {t!("delivery-detail-attempt-world")} }
                                th { scope: "col", {t!("delivery-detail-attempt-recipient")} }
                                th { scope: "col", {t!("delivery-detail-attempt-subject")} }
                                th { scope: "col", {t!("delivery-detail-attempt-digest")} }
                                th { scope: "col", {t!("delivery-detail-attempt-amounts")} }
                                th { scope: "col", {t!("delivery-detail-attempt-mid")} }
                            }
                        }
                        tbody {
                            for attempt in attempts {
                                tr { key: "{attempt.fencing_version}-{attempt.operation}",
                                    td {
                                        if attempt.operation == "send" {
                                            {t!("delivery-detail-attempt-send")}
                                        } else {
                                            {t!("delivery-detail-attempt-take")}
                                        }
                                    }
                                    td { "{format_admin_time(&attempt.sending_at)}" }
                                    td { "{attempt.fencing_version}" }
                                    td { "{admin_world_label(&attempt.world_id)}" }
                                    td {
                                        TruncatedText {
                                            value: attempt.recipient.clone().unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td {
                                        TruncatedText {
                                            value: attempt.subject.clone().unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td {
                                        TruncatedText {
                                            value: attempt.body_digest.clone().unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td { "{attempt_amounts(&attempt)}" }
                                    td {
                                        TruncatedText {
                                            value: attempt_mail_id(&attempt),
                                            portal: true,
                                        }
                                    }
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
pub(super) fn EvidenceSection(evidence: Vec<DeliveryEvidence>) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-evidence-title",
            h2 { id: "delivery-detail-evidence-title", {t!("delivery-detail-evidence")} }
            if evidence.is_empty() {
                p { class: "admin-panel__note", {t!("delivery-detail-evidence-empty")} }
            } else {
                div { class: "admin-shell__table-wrap",
                    table { class: "admin-shell__table admin-delivery-detail__table",
                        aria_label: t!("delivery-detail-evidence"),
                        TableColumns { layout: "admin-delivery-evidence", widths: vec![10, 14, 12, 20, 14, 14, 16] }
                        thead {
                            tr {
                                th { scope: "col", {t!("delivery-detail-evidence-kind")} }
                                th { scope: "col", {t!("delivery-detail-evidence-reference")} }
                                th { scope: "col", {t!("delivery-detail-evidence-observed")} }
                                th { scope: "col", {t!("delivery-detail-evidence-subject")} }
                                th { scope: "col", {t!("delivery-detail-evidence-digest")} }
                                th { scope: "col", {t!("delivery-detail-evidence-custody")} }
                                th { scope: "col", {t!("delivery-detail-evidence-receipt")} }
                            }
                        }
                        tbody {
                            for item in evidence {
                                tr { key: "{item.reference}",
                                    td { "{evidence_kind_label(&item.kind)}" }
                                    td { TruncatedText { value: item.reference.clone(), portal: true } }
                                    td {
                                        "{format_admin_time(&item.observed_at.clone().unwrap_or_default())}"
                                        if let Some(sent_at) = item.sent_at.clone() {
                                            div { class: "admin-delivery-detail__sub",
                                                "{format_admin_time(&sent_at)}"
                                            }
                                        }
                                    }
                                    td {
                                        TruncatedText {
                                            value: item.subject.clone().unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td {
                                        TruncatedText {
                                            value: item.body_digest.clone().unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td { "{evidence_custody(&item)}" }
                                    td {
                                        TruncatedText {
                                            value: item.receipt.unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
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
pub(super) fn CustodySection(custody: DeliveryCustody) -> Element {
    let observed = match &custody.observed_at {
        Some(value) => format_admin_time(value),
        None => t!("delivery-detail-custody-unobserved"),
    };
    let collected = match custody.collected {
        Some(true) => t!("delivery-detail-yes"),
        Some(false) => t!("delivery-detail-no"),
        None => t!("delivery-detail-custody-unobserved"),
    };
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-custody-title",
            h2 { id: "delivery-detail-custody-title", {t!("delivery-detail-custody")} }
            dl { class: "admin-delivery-detail__grid",
                dt { {t!("delivery-detail-custody-collected")} }
                dd { "{collected}" }
                dt { {t!("delivery-detail-custody-cod-pending")} }
                dd {
                    "{custody.cod_pending.map_or_else(|| t!(\"value-empty\"), |value| value.to_string())}"
                }
                dt { {t!("delivery-detail-custody-remaining-credits")} }
                dd {
                    "{custody.remaining_credits.map_or_else(|| t!(\"value-empty\"), |value| value.to_string())}"
                }
                dt { {t!("delivery-detail-custody-observed")} }
                dd { "{observed}" }
                dt { {t!("delivery-detail-custody-confirmed")} }
                dd {
                    if custody.operator_confirmed_collected {
                        {t!("delivery-detail-yes")}
                    } else {
                        {t!("delivery-detail-no")}
                    }
                }
                dt { {t!("delivery-detail-custody-receipt")} }
                dd {
                    if let Some(receipt) = custody.receipt {
                        pre { class: "admin-delivery-detail__receipt", "{receipt}" }
                    } else {
                        {t!("value-empty")}
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn DispositionSection(dispositions: Vec<DeliveryDisposition>) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-dispositions-title",
            h2 { id: "delivery-detail-dispositions-title", {t!("delivery-detail-dispositions")} }
            if dispositions.is_empty() {
                p { class: "admin-panel__note", {t!("delivery-detail-dispositions-empty")} }
            } else {
                div { class: "admin-shell__table-wrap",
                    table { class: "admin-shell__table admin-delivery-detail__table",
                        aria_label: t!("delivery-detail-dispositions"),
                        TableColumns { layout: "admin-delivery-dispositions", widths: vec![16, 16, 22, 14, 16, 16] }
                        thead {
                            tr {
                                th { scope: "col", {t!("delivery-detail-disposition-kind")} }
                                th { scope: "col", {t!("delivery-detail-disposition-time")} }
                                th { scope: "col", {t!("delivery-detail-disposition-command")} }
                                th { scope: "col", {t!("delivery-detail-disposition-status")} }
                                th { scope: "col", {t!("delivery-detail-disposition-failure")} }
                                th { scope: "col", {t!("delivery-detail-disposition-actor")} }
                            }
                        }
                        tbody {
                            for item in dispositions {
                                tr { key: "{item.command_id}",
                                    td { "{disposition_kind_label(&item.kind)}" }
                                    td { "{format_admin_time(&item.occurred_at)}" }
                                    td { TruncatedText { value: item.command_id.clone(), portal: true } }
                                    td { "{disposition_status(&item)}" }
                                    td {
                                        TruncatedText {
                                            value: item.failure_code.clone()
                                                .or_else(|| item.external_reference.clone())
                                                .unwrap_or_else(|| t!("value-empty")),
                                            portal: true,
                                        }
                                    }
                                    td {
                                        TruncatedText {
                                            value: disposition_actor(&item),
                                            portal: true,
                                        }
                                    }
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
pub(super) fn ObligationSection(
    obligation: Option<DeliveryObligation>,
    recovery: Option<DeliveryRecovery>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-detail-obligation-title",
            h2 { id: "delivery-detail-obligation-title", {t!("delivery-detail-obligation")} }
            if let Some(obligation) = obligation {
                dl { class: "admin-delivery-detail__grid",
                    dt { {t!("delivery-detail-obligation-id")} }
                    dd { TruncatedText { value: obligation.obligation_id.clone(), portal: true } }
                    dt { {t!("delivery-detail-obligation-auction")} }
                    dd { TruncatedText { value: obligation.auction_id.clone(), portal: true } }
                    dt { {t!("delivery-detail-obligation-lot")} }
                    dd { TruncatedText { value: obligation.lot_id.clone(), portal: true } }
                    dt { {t!("delivery-detail-obligation-world")} }
                    dd { "{admin_world_label(&obligation.world_id)}" }
                    dt { {t!("delivery-detail-obligation-kind")} }
                    dd { "{obligation.obligation_kind}" }
                    dt { {t!("delivery-detail-obligation-state")} }
                    dd { "{obligation.business_state}" }
                    dt { {t!("delivery-detail-obligation-amount")} }
                    dd { "{obligation.amount}" }
                    dt { {t!("delivery-detail-obligation-fulfillment")} }
                    dd {
                        "{obligation.fulfillment_state.clone().unwrap_or_else(|| t!(\"value-empty\"))}"
                    }
                    dt { {t!("delivery-detail-obligation-fulfillment-error")} }
                    dd {
                        "{obligation.fulfillment_error_code.clone().unwrap_or_else(|| t!(\"value-empty\"))}"
                    }
                }
            } else {
                p { class: "admin-panel__note", {t!("delivery-detail-obligation-none")} }
            }
        }
        section { class: "admin-panel", aria_labelledby: "delivery-detail-recovery-title",
            h2 { id: "delivery-detail-recovery-title", {t!("delivery-detail-recovery")} }
            if let Some(recovery) = recovery {
                dl { class: "admin-delivery-detail__grid",
                    dt { {t!("delivery-detail-recovery-id")} }
                    dd { TruncatedText { value: recovery.recovery_id.clone(), portal: true } }
                    dt { {t!("delivery-detail-recovery-stage")} }
                    dd { "{recovery.stage}" }
                    dt { {t!("delivery-detail-recovery-resolution")} }
                    dd { "{recovery.resolution.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-detail-recovery-deadline")} }
                    dd { "{format_admin_time(&recovery.payment_deadline_at)}" }
                    dt { {t!("delivery-detail-recovery-payment-obligation")} }
                    dd { TruncatedText { value: recovery.payment_obligation_id.clone(), portal: true } }
                    dt { {t!("delivery-detail-recovery-blocking")} }
                    dd { "{recovery.blocking_code.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-detail-recovery-manual-delivery")} }
                    dd {
                        "{recovery.manual_reconcile_delivery_id.clone().unwrap_or_else(|| t!(\"value-empty\"))}"
                    }
                }
            } else {
                p { class: "admin-panel__note", {t!("delivery-detail-recovery-none")} }
            }
        }
    }
}

fn attempt_amounts(attempt: &DeliveryAttempt) -> String {
    let credits = attempt.credits.map_or_else(|| t!("value-empty"), |value| value.to_string());
    let cod = attempt.cod.map_or_else(|| t!("value-empty"), |value| value.to_string());
    format!("{credits} / {cod}")
}

fn attempt_mail_id(attempt: &DeliveryAttempt) -> String {
    let external = attempt.external_mail_id.clone().unwrap_or_default();
    let bound = attempt.bound_sent_mail_id.clone().unwrap_or_default();
    match (external.is_empty(), bound.is_empty()) {
        (true, true) => t!("value-empty"),
        (false, true) => external,
        (true, false) => format!("{bound} ({})", t!("delivery-detail-attempt-bound")),
        (false, false) => format!("{external} ({bound})"),
    }
}

fn evidence_custody(item: &DeliveryEvidence) -> String {
    let collected = match item.collected {
        Some(true) => t!("delivery-detail-yes"),
        Some(false) => t!("delivery-detail-no"),
        None => t!("delivery-detail-custody-unobserved"),
    };
    let credits =
        item.remaining_credits.map_or_else(|| t!("value-empty"), |value| value.to_string());
    let cod = item.cod_pending.map_or_else(|| t!("value-empty"), |value| value.to_string());
    format!("{collected} · {credits} / {cod}")
}

fn disposition_kind_label(kind: &str) -> String {
    match kind {
        "release" => t!("delivery-detail-disposition-kind-release"),
        "retry_failed" => t!("delivery-detail-disposition-kind-retry"),
        "reconcile" => t!("delivery-detail-disposition-kind-reconcile"),
        "sent_operator_confirmed" => t!("delivery-detail-disposition-kind-sent-confirmed"),
        "operator_acknowledged" => t!("delivery-detail-disposition-kind-operator-acknowledged"),
        "auto_sent_proof" => t!("delivery-detail-disposition-kind-auto-sent-proof"),
        "operator_sent_confirmation" => {
            t!("delivery-detail-disposition-kind-operator-sent-confirmation")
        }
        "admin_failed" => t!("delivery-detail-disposition-kind-admin-failed"),
        "equipment_loss" => t!("delivery-detail-disposition-kind-equipment-loss"),
        "cod_returned" => t!("delivery-detail-disposition-kind-cod-returned"),
        "legacy_self" => t!("delivery-detail-disposition-kind-legacy-self"),
        "preflight_retired" => t!("delivery-detail-disposition-kind-preflight-retired"),
        other => other.to_owned(),
    }
}

fn disposition_status(item: &DeliveryDisposition) -> String {
    let status = item.status.as_deref().map_or_else(|| t!("value-empty"), delivery_status_label);
    match item.fencing_version {
        Some(version) => format!("{status} · v{version}"),
        None => status,
    }
}

fn disposition_actor(item: &DeliveryDisposition) -> String {
    let actor = item.actor.clone().unwrap_or_else(|| t!("value-empty"));
    match item.reason.as_deref() {
        Some(reason) if !reason.is_empty() => format!("{actor} · {reason}"),
        _ => actor,
    }
}
