//! 预览与确认动作，以及服务端预览结果的展示。
//!
//! 预览只发送本地校验通过的安全投影且无副作用；确认沿用详情页命令宿主的稳定
//! `command_id`，服务端仍会重新判定准入，不把预览当作成功证明。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::state::{SentProofDraft, SentProofSubmission};
use super::validation::build_evidence;
use crate::api::{DeliveryDetail, DeliverySentProofPreview, preview_delivery_sent_proof};
use crate::surface::admin_api_error_message;
use crate::surface::pending::set_if_alive;

/// 组装并发送无副作用预览；本地形状不合格时只记录问题码，不发请求。
pub(super) fn run_preview(
    detail: &DeliveryDetail,
    busy: bool,
    draft: Signal<SentProofDraft>,
    mut preview: Signal<Option<Result<DeliverySentProofPreview, String>>>,
    mut previewing: Signal<bool>,
) {
    if busy || previewing() {
        return;
    }
    let current = draft();
    match build_evidence(detail, &current) {
        Err(issue) => preview.set(Some(Err(issue.message()))),
        Ok(evidence) => {
            let reference = current.evidence_reference.trim().to_owned();
            let delivery_id = detail.delivery.id.clone();
            preview.set(None);
            previewing.set(true);
            spawn(async move {
                let result = preview_delivery_sent_proof(delivery_id, reference, evidence)
                    .await
                    .map_err(|failure| failure.code);
                set_if_alive(preview, Some(result));
                set_if_alive(previewing, false);
            });
        }
    }
}

/// 把一次性提交转交给命令宿主。
pub(super) fn submit_proof(
    detail: &DeliveryDetail,
    draft: Signal<SentProofDraft>,
    accept_collected: Signal<bool>,
    accept_attempts: Signal<bool>,
    can_confirm: bool,
    on_submit: EventHandler<SentProofSubmission>,
) {
    if !can_confirm {
        return;
    }
    let current = draft();
    let Ok(evidence) = build_evidence(detail, &current) else {
        return;
    };
    on_submit.call(SentProofSubmission {
        evidence_reference: current.evidence_reference.trim().to_owned(),
        evidence,
        reason: current.reason.trim().to_owned(),
        expected_fencing_version: detail.delivery.fencing_version,
        accept_collected_assets: accept_collected(),
        accept_attempt_history: accept_attempts(),
    });
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
        other => other.to_owned(),
    }
}

/// 预览结果面板：只展示服务端闭集判定，勾选项由服务端要求驱动。
pub(super) fn preview_panel(
    result: Result<DeliverySentProofPreview, String>,
    busy: bool,
    mut accept_collected: Signal<bool>,
    mut accept_attempts: Signal<bool>,
) -> Element {
    match result {
        Ok(preview) => rsx! {
            section { class: "admin-delivery-sent-proof__preview", role: "status",
                if preview.eligible {
                    p { {t!("delivery-sent-proof-eligible")} }
                } else {
                    p { class: "admin-delivery-reconcile__warning",
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
                    label { class: "admin-delivery-sent-proof__check",
                        input {
                            r#type: "checkbox",
                            disabled: busy,
                            checked: accept_collected(),
                            onchange: move |event| accept_collected.set(event.checked()),
                        }
                        {t!("delivery-sent-proof-accept-collected")}
                    }
                }
                if preview.requires_attempt_history_acceptance {
                    label { class: "admin-delivery-sent-proof__check",
                        input {
                            r#type: "checkbox",
                            disabled: busy,
                            checked: accept_attempts(),
                            onchange: move |event| accept_attempts.set(event.checked()),
                        }
                        {t!("delivery-sent-proof-accept-attempts")}
                    }
                }
            }
        },
        Err(code) => rsx! {
            p { class: "admin-delivery-reconcile__warning", role: "alert",
                {admin_api_error_message(&code)}
            }
        },
    }
}
