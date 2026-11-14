//! Admin 投递详情页：冻结尝试、可引用 Sent 证据、领取状态、处置与来源义务。
//!
//! 只读聚合与人工 Sent 证明入口；所有字段都来自 `auction-contracts` 的安全投影，
//! 页面不展示投递 payload、正文、Cookie 或登录 key，也不在浏览器重建状态机。

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use crate::api::{
    DeliverySentProofResult, SentProofCommand, acknowledge_delivery, confirm_delivery_sent_proof,
    fetch_delivery_detail, valid_uuid,
};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

use super::super::Route;
use super::sent_proof::{SentProofDialog, SentProofSubmission, selectable_evidence};

mod sections;

use sections::{
    AttemptSection, CustodySection, DeliverySummary, DispositionSection, EvidenceSection,
    ObligationSection,
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
