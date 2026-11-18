//! 预览与确认动作，以及服务端预览结果的展示。
//!
//! 预览只发送本地校验通过的安全投影且无副作用；确认沿用详情页命令宿主的稳定
//! `command_id`，服务端仍会重新判定准入，不把预览当作成功证明。

use auction_web_shared::{a11y::focus_element_by_id_after_render, t};
use dioxus::prelude::*;

use super::state::{
    ReviewedSentProof, SentProofDraft, SentProofPreviewError, SentProofRequest, SentProofReview,
    SentProofSubmission,
};
use super::validation::{build_evidence, valid_reason};
use crate::api::{
    AdminApiFailure, DeliveryDetail, DeliverySentProofPreview, preview_delivery_sent_proof,
};
use crate::surface::admin_api_error_message;

/// 组装并发送无副作用预览；本地形状不合格时只记录问题码，不发请求。
pub(super) fn run_preview(
    detail: &DeliveryDetail,
    busy: bool,
    draft: Signal<SentProofDraft>,
    mut review: Signal<SentProofReview>,
    on_stale: EventHandler<()>,
) {
    if busy || review.peek().pending {
        return;
    }
    let current = draft();
    match build_evidence(detail, &current) {
        Err(issue) => {
            let mut review = review.write();
            review.clear();
            review.outcome = Some(Err(SentProofPreviewError::Local(issue)));
            focus_element_by_id_after_render(issue.field_id());
        }
        Ok(evidence) => {
            let request = SentProofRequest {
                delivery_id: detail.delivery.id.clone(),
                fencing_version: detail.delivery.fencing_version,
                evidence_reference: current.evidence_reference.trim().to_owned(),
                evidence,
            };
            let revision = review.write().begin();
            spawn(async move {
                let result = request_preview(&request).await;
                let stale = review
                    .try_write()
                    .is_ok_and(|mut review| review.complete(revision, request, result));
                if stale {
                    on_stale.call(());
                }
            });
        }
    }
}

async fn request_preview(
    request: &SentProofRequest,
) -> Result<DeliverySentProofPreview, AdminApiFailure> {
    #[cfg(test)]
    if let Some(response) = super::tests::preview_response(request).await {
        return response;
    }
    preview_delivery_sent_proof(
        request.delivery_id.clone(),
        request.evidence_reference.clone(),
        request.evidence.clone(),
    )
    .await
}

/// 把一次性提交转交给命令宿主。
pub(super) fn submit_proof(
    delivery_id: &str,
    fencing_version: i64,
    draft: Signal<SentProofDraft>,
    review: Signal<SentProofReview>,
    on_submit: EventHandler<SentProofSubmission>,
) {
    let reviewed = review.peek();
    let Some(Ok(proof)) = &reviewed.outcome else {
        return;
    };
    let draft = draft.peek();
    if !proof.can_confirm(delivery_id, fencing_version) || !valid_reason(&draft.reason) {
        return;
    }
    let submission = proof.submission(&draft.reason);
    drop(draft);
    drop(reviewed);
    on_submit.call(submission);
}

/// 服务端闭集 blocker 的译文；未知码原样显示，不猜测语义。
fn blocker_label(code: &str) -> String {
    match code {
        "unsupported_delivery_scope" => t!("delivery-sent-proof-blocker-scope"),
        "requires_unclaimed_write_unknown_or_admin_failure" => {
            t!("delivery-sent-proof-blocker-requires-unknown")
        }
        "fulfillment_identity_or_claim_conflict" => t!("delivery-sent-proof-blocker-fulfillment"),
        "conflicting_recovery_disposition" => t!("delivery-sent-proof-blocker-recovery"),
        "existing_binding_retirement_or_conflicting_attempt" => {
            t!("delivery-sent-proof-blocker-binding")
        }
        "requires_unique_ready_sender_funding_account" => {
            t!("delivery-sent-proof-blocker-sender-account")
        }
        "sender_player_name_mismatch" => t!("delivery-sent-proof-blocker-sender-name"),
        "frozen_credential_retired_or_changed" => t!("delivery-sent-proof-blocker-credential"),
        "sent_minute_outside_frozen_window" => t!("delivery-sent-proof-blocker-minute-window"),
        "requires_exact_unknown_seller_reservation" => {
            t!("delivery-sent-proof-blocker-seller-reservation")
        }
        "frozen_identity_body_money_or_assets_mismatch" => {
            t!("delivery-sent-proof-blocker-frozen-identity")
        }
        "missing_observed_equipment_ids" => t!("delivery-sent-proof-missing-observed-ids"),
        "observed_equipment_ids_mismatch" => t!("delivery-sent-proof-ids-mismatch"),
        other => other.to_owned(),
    }
}

/// 预览结果面板：只展示服务端闭集判定，勾选项由服务端要求驱动。
pub(super) fn preview_panel(
    review_state: &SentProofReview,
    busy: bool,
    mut review: Signal<SentProofReview>,
) -> Element {
    let revision = review_state.revision();
    match &review_state.outcome {
        Some(Ok(ReviewedSentProof { preview, accepted, .. })) => rsx! {
            section { class: "admin-delivery-sent-proof__preview", role: "status",
                if preview.eligible {
                    p { {t!("delivery-sent-proof-eligible")} }
                } else {
                    p { class: "action-feedback action-feedback--error",
                        {t!("delivery-sent-proof-blocked")}
                    }
                }
                if preview.corrects_admin_failure {
                    p { class: "admin-panel__note",
                        {t!("delivery-sent-proof-corrects-failure")}
                    }
                }
                if !preview.blockers.is_empty() {
                    ul { class: "admin-delivery-sent-proof__blockers",
                        for blocker in preview.blockers.iter() {
                            li { key: "{blocker}", {blocker_label(blocker)} }
                        }
                    }
                }
                if preview.requires_collected_asset_acceptance {
                    label { class: "admin-delivery-sent-proof__check", r#for: "delivery-sent-accept-collected",
                        input {
                            id: "delivery-sent-accept-collected",
                            r#type: "checkbox",
                            name: "accept_collected_assets",
                            disabled: busy,
                            checked: accepted.collected_assets,
                            onchange: move |event| {
                                if let Some(accepted) = review.write().acceptance(revision) {
                                    accepted.collected_assets = event.checked();
                                }
                            },
                        }
                        {t!("delivery-sent-proof-accept-collected")}
                    }
                }
                if preview.prior_unbound_attempts == Some(true) {
                    label { class: "admin-delivery-sent-proof__check", r#for: "delivery-sent-accept-prior-unbound",
                        input {
                            id: "delivery-sent-accept-prior-unbound",
                            r#type: "checkbox",
                            name: "accept_prior_unbound_attempts",
                            disabled: busy,
                            checked: accepted.prior_unbound_attempts,
                            onchange: move |event| {
                                if let Some(accepted) = review.write().acceptance(revision) {
                                    accepted.prior_unbound_attempts = event.checked();
                                }
                            },
                        }
                        {t!("delivery-sent-proof-accept-attempts")}
                    }
                }
                if preview.sent_before_current_attempt == Some(true) {
                    label { class: "admin-delivery-sent-proof__check", r#for: "delivery-sent-accept-before-current",
                        input {
                            id: "delivery-sent-accept-before-current",
                            r#type: "checkbox",
                            name: "accept_sent_before_current_attempt",
                            disabled: busy,
                            checked: accepted.sent_before_current_attempt,
                            onchange: move |event| {
                                if let Some(accepted) = review.write().acceptance(revision) {
                                    accepted.sent_before_current_attempt = event.checked();
                                }
                            },
                        }
                        {t!("delivery-sent-proof-accept-before-current")}
                    }
                }
            }
        },
        Some(Err(SentProofPreviewError::Api(failure))) => rsx! {
            p { class: "action-feedback action-feedback--error", role: "alert",
                {admin_api_error_message(&failure.code)}
            }
        },
        _ => rsx! {},
    }
}
