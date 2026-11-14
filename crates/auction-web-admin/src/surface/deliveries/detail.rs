//! Admin 投递详情页：冻结尝试、可引用 Sent 证据、领取状态、处置与来源义务。
//!
//! 只读聚合与人工 Sent 证明入口；所有字段都来自 `auction-contracts` 的安全投影，
//! 页面不展示投递 payload、正文、Cookie 或登录 key，也不在浏览器重建状态机。

use auction_web_shared::{AsyncState, AsyncStateKind, TableColumns, TruncatedText, t};
use dioxus::prelude::*;

use crate::api::{
    DeliveryAttempt, DeliveryCustody, DeliveryDetail, DeliveryDisposition, DeliveryEvidence,
    DeliveryObligation, DeliveryRecovery, DeliverySentProofResult, SentProofCommand,
    acknowledge_delivery, confirm_delivery_sent_proof, fetch_delivery_detail, valid_uuid,
};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{
    AdminActionFeedback, admin_api_error_message, admin_world_label, format_admin_time,
};

use super::super::Route;
use super::helpers::{delivery_display_type, delivery_status_label};
use super::sent_proof::{
    SentProofDialog, SentProofSubmission, evidence_kind_label, selectable_evidence,
};

/// 路由复用的是同一组件实例，因此按投递身份重挂载资源与一次性状态。
pub(crate) fn detail_page(delivery_id: String) -> Element {
    rsx! {
        for delivery_id in [delivery_id] {
            DeliveryDetailPage { key: "{delivery_id}", delivery_id }
        }
    }
}

#[component]
fn DeliveryDetailPage(delivery_id: String) -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let commands = use_pending_commands(refresh);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut sent_open = use_signal(|| false);
    let mut sent_submitted = use_signal(|| false);
    let acknowledge_reason = use_signal(String::new);
    let acknowledged = use_signal(|| false);

    let detail = use_resource(use_reactive((&delivery_id,), move |(delivery_id,)| {
        let _ = refresh();
        async move {
            let result = fetch_delivery_detail(delivery_id.clone()).await;
            (delivery_id, result)
        }
    }));
    let loading = detail.state().cloned() != UseResourceState::Ready;
    // 切换投递或刷新期间不展示上一轮结果，避免把旧证据挂在新投递下。
    let loaded = if loading {
        None
    } else {
        detail
            .read()
            .as_ref()
            .filter(|(loaded_id, _)| loaded_id == &delivery_id)
            .map(|(_, result)| result.clone())
    };

    let confirm_delivery_id = delivery_id.clone();
    let confirm_sent_proof = move |submission: SentProofSubmission| {
        let entity = confirm_delivery_id.clone();
        if commands.any_busy() || commands.contains(&entity) {
            return;
        }
        notice.set(None);
        sent_submitted.set(false);
        let submitted = commands.submit(
            entity.clone(),
            t!("delivery-sent-proof-confirm"),
            move |command_id| {
                let delivery_id = entity.clone();
                let submission = submission.clone();
                async move {
                    confirm_delivery_sent_proof(SentProofCommand {
                        command_id,
                        delivery_id,
                        expected_fencing_version: submission.expected_fencing_version,
                        reason: submission.reason,
                        evidence_reference: submission.evidence_reference,
                        evidence: submission.evidence,
                        accept_collected_assets: submission.accept_collected_assets,
                        accept_attempt_history: submission.accept_attempt_history,
                    })
                    .await
                    .map_err(|failure| failure.code)
                }
            },
            move |_: DeliverySentProofResult| {
                set_if_alive(notice, Some(AdminActionFeedback::Success));
                set_if_alive(sent_open, false);
                set_if_alive(sent_submitted, true);
            },
            move |code| {
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        if submitted {
            sent_open.set(false);
        }
    };

    let acknowledge_delivery_id = delivery_id.clone();
    let acknowledge_fencing_version = loaded
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map_or(0, |detail| detail.delivery.fencing_version);
    let acknowledge = move |_| {
        let entity = acknowledge_delivery_id.clone();
        let reason = acknowledge_reason().trim().to_owned();
        if reason.is_empty() || commands.any_busy() || commands.contains(&entity) {
            return;
        }
        notice.set(None);
        let submitted = commands.submit(
            entity.clone(),
            t!("delivery-detail-acknowledge-submit"),
            move |command_id| {
                let delivery_id = entity.clone();
                let reason = reason.clone();
                async move {
                    acknowledge_delivery(
                        command_id,
                        delivery_id,
                        acknowledge_fencing_version,
                        reason,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|failure| failure.code)
                }
            },
            move |_: ()| {
                set_if_alive(acknowledged, true);
                set_if_alive(acknowledge_reason, String::new());
                let mut refresh = refresh;
                refresh.set(refresh().saturating_add(1));
            },
            move |code| {
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        let _ = submitted;
    };
    let acknowledge_entry = loaded.clone().and_then(std::result::Result::ok).map_or_else(|| rsx! {}, |detail| {
        let already = acknowledged()
            || detail
                .dispositions
                .iter()
                .any(|value| value.kind == "operator_acknowledged");
        let eligible = detail.delivery.status == "failed" && !already;
        if !eligible {
            return rsx! {};
        }
        let mut reason = acknowledge_reason;
        let mut submit = acknowledge.clone();
        rsx! {
            section { class: "admin-panel admin-delivery-acknowledge",
                aria_labelledby: "delivery-acknowledge-title",
                h2 { id: "delivery-acknowledge-title", {t!("delivery-detail-acknowledge-title")} }
                p { class: "admin-panel__note", {t!("delivery-detail-acknowledge-intro")} }
                label { class: "admin-field",
                    span { {t!("delivery-detail-acknowledge-reason")} }
                    textarea {
                        rows: "3",
                        maxlength: "2048",
                        value: "{reason}",
                        oninput: move |event| reason.set(event.value()),
                    }
                }
                button {
                    r#type: "button",
                    class: "admin-btn-primary",
                    disabled: commands.any_busy() || reason().trim().is_empty(),
                    onclick: move |_| submit(()),
                    {t!("delivery-detail-acknowledge-submit")}
                }
            }
        }
    });

    // 证明入口只有在存在可引用证据且投递处于可准入状态时出现；服务端仍会重新判定。
    let sent_entry = loaded.clone().and_then(std::result::Result::ok).map(|detail| {
        let candidates = !selectable_evidence(&detail).is_empty();
        let can_prove =
            matches!(detail.delivery.status.as_str(), "manual_reconcile" | "failed") && candidates;
        rsx! {
            section { class: "admin-panel admin-delivery-sent-proof-entry",
                aria_labelledby: "delivery-sent-proof-entry-title",
                h2 { id: "delivery-sent-proof-entry-title", {t!("delivery-detail-sent-proof")} }
                p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-intro")} }
                if can_prove {
                    button {
                        r#type: "button",
                        class: "admin-btn-primary",
                        disabled: commands.any_busy(),
                        onclick: move |_| {
                            notice.set(None);
                            sent_open.set(true);
                        },
                        {t!("delivery-detail-sent-proof-open")}
                    }
                } else if !candidates {
                    p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-no-evidence")} }
                } else {
                    p { class: "admin-panel__note", {t!("delivery-detail-sent-proof-not-applicable")} }
                }
            }
            if sent_submitted() {
                p { class: "admin-delivery-status", role: "status",
                    {t!("delivery-sent-proof-submitted")}
                }
            }
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "admin-delivery-status"
                    } else {
                        "admin-delivery-error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("delivery-sent-proof-submitted"))}
                }
            }
            if sent_open() {
                SentProofDialog {
                    detail: detail,
                    busy: commands.any_busy(),
                    on_submit: confirm_sent_proof,
                    on_cancel: move |_| sent_open.set(false),
                }
            }
        }
    });

    rsx! {
        section { class: "admin-delivery-detail", aria_labelledby: "delivery-detail-title",
            div { class: "admin-delivery-detail__head",
                h1 { id: "delivery-detail-title", {t!("delivery-detail-title")} }
                Link { to: Route::Deliveries {}, class: "admin-delivery-detail__back",
                    {t!("delivery-detail-back")}
                }
            }
            PendingCommandPanel { owner: commands }
            match loaded {
                None if !valid_uuid(&delivery_id) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message("invalid_delivery_id"),
                        detail: "",
                    }
                },
                None => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Loading,
                        title: t!("delivery-detail-loading"),
                        detail: "",
                    }
                },
                Some(Err(error)) => rsx! {
                    AsyncState {
                        kind: AsyncStateKind::Error,
                        title: admin_api_error_message(&error),
                        detail: "",
                        retry_label: t!("admin-retry"),
                        on_retry: move |_| refresh.set(refresh().saturating_add(1)),
                    }
                },
                Some(Ok(detail)) => rsx! {
                    DeliverySummary { detail: detail.clone() }
                    AttemptSection { attempts: detail.attempts.clone() }
                    EvidenceSection { evidence: detail.evidence.clone() }
                    CustodySection { custody: detail.custody.clone() }
                    DispositionSection { dispositions: detail.dispositions.clone() }
                    ObligationSection {
                        obligation: detail.obligation.clone(),
                        recovery: detail.recovery,
                    }
                },
            }
            if acknowledged() {
                p { class: "admin-delivery-status", role: "status",
                    {t!("delivery-detail-acknowledge-success")}
                }
            }
            {acknowledge_entry}
            {sent_entry}
        }
    }
}

#[component]
fn DeliverySummary(detail: DeliveryDetail) -> Element {
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
fn AttemptSection(attempts: Vec<DeliveryAttempt>) -> Element {
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
fn EvidenceSection(evidence: Vec<DeliveryEvidence>) -> Element {
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
fn CustodySection(custody: DeliveryCustody) -> Element {
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
fn DispositionSection(dispositions: Vec<DeliveryDisposition>) -> Element {
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
fn ObligationSection(
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
