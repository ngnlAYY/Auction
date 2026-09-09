//! Admin 邮件入站详情安全投影。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::AdminMailIntakeDetail;
use crate::app::surface::admin_api_error_message;

use super::super::format_admin_time;
use super::helpers::{mail_kind_label, mail_status_class, mail_status_label};

#[component]
pub(super) fn MailIntakeDetailPanel(
    visible: bool,
    detail: Option<AdminMailIntakeDetail>,
    loading: bool,
    error: Option<String>,
    mut refresh: Signal<u64>,
    delivery_notice: Signal<Option<(String, super::super::AdminActionFeedback)>>,
    on_close: EventHandler<()>,
) -> Element {
    if !visible {
        return rsx! {};
    }

    if loading {
        return rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-mail-detail-title"),
                detail: t!("admin-mail-detail-loading"),
            }
        };
    }

    if let Some(code) = error {
        return rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("admin-mail-detail-title"),
                detail: admin_api_error_message(&code),
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
            // 共享面板只承载一个重试按钮，关闭入口单独保留，行为与旧版一致。
            button { r#type: "button", onclick: move |_| on_close.call(()), {t!("admin-mail-close-detail")} }
        };
    }

    let Some(detail) = detail else {
        return rsx! {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-mail-detail-title"),
                detail: t!("admin-mail-no-selection"),
            }
        };
    };

    let metadata = detail.metadata;
    let warnings = if metadata.warning_codes.is_empty() {
        t!("value-empty")
    } else {
        metadata.warning_codes.join(", ")
    };
    let delivery_ids = if metadata.delivery_ids.is_empty() {
        t!("value-empty")
    } else {
        metadata.delivery_ids.join(", ")
    };
    let original = detail.message_display;
    let equipment_diagnostic = detail.equipment_diagnostic;
    let sender = detail.sender.clone().unwrap_or_else(|| t!("admin-mail-original-not-retained"));
    let subject = detail.subject.clone().unwrap_or_else(|| t!("admin-mail-original-not-retained"));
    let diagnostic_detail = equipment_diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.detail_code.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_http_status = equipment_diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.http_status.map(|value| value.to_string()))
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_elapsed = equipment_diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.elapsed_millis.map(|value| value.to_string()))
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_evidence_id = equipment_diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.evidence_id.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_retention_error = equipment_diagnostic
        .as_ref()
        .and_then(|diagnostic| diagnostic.retention_error.clone())
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_stage = equipment_diagnostic
        .as_ref()
        .map(|diagnostic| equipment_diagnostic_stage_label(&diagnostic.stage))
        .unwrap_or_else(|| t!("value-empty"));
    let diagnostic_evidence_status = equipment_diagnostic
        .as_ref()
        .map(|diagnostic| equipment_evidence_status_label(&diagnostic.evidence_status))
        .unwrap_or_else(|| t!("value-empty"));

    rsx! {
        section { class: "admin-panel admin-mail-detail", aria_labelledby: "admin-mail-detail-title",
            header { class: "admin-mail-detail__header",
                h2 { id: "admin-mail-detail-title", {t!("admin-mail-detail-title")} }
                button { r#type: "button", onclick: move |_| on_close.call(()), {t!("admin-mail-close-detail")} }
            }
            dl { class: "admin-mail-detail__grid",
                dt { {t!("admin-mail-intake-id")} }
                dd { "{detail.id}" }
                dt { {t!("admin-mail-external-id")} }
                dd { "{detail.external_mail_id}" }
                dt { {t!("admin-mail-mailbox")} }
                dd { "{detail.mailbox_key}" }
                dt { {t!("admin-mail-intake-from")} }
                dd { "{sender}" }
                dt { {t!("admin-mail-intake-subject")} }
                dd { "{subject}" }
                dt { {t!("admin-mail-intake-world")} }
                dd { "{detail.world_id}" }
                dt { {t!("admin-mail-intake-kind")} }
                dd { "{mail_kind_label(&detail.kind)}" }
                dt { {t!("admin-mail-intake-status")} }
                dd {
                    span { class: mail_status_class(&detail.status),
                        {mail_status_label(&detail.status)}
                    }
                }
                dt { {t!("admin-mail-intake-decision")} }
                dd { "{detail.decision_code}" }
                dt { {t!("admin-mail-intake-received")} }
                dd { "{format_admin_time(&detail.received_at)}" }
                dt { {t!("admin-mail-processed")} }
                dd { "{format_admin_time(&detail.processed_at)}" }
                dt { {t!("admin-mail-auction-id")} }
                dd { "{metadata.auction_id}" }
                dt { {t!("admin-mail-submission-id")} }
                dd { "{metadata.submission_id}" }
                dt { {t!("admin-mail-user-id")} }
                dd { "{metadata.user_id}" }
                dt { {t!("admin-mail-added-count")} }
                dd { "{metadata.added_count}" }
                dt { {t!("admin-mail-existing-count")} }
                dd { "{metadata.existing_count}" }
                dt { {t!("admin-mail-preview-revision")} }
                dd { "{metadata.preview_revision}" }
                dt { {t!("admin-mail-attachment-count")} }
                dd { "{metadata.attachment_count}" }
                dt { {t!("admin-mail-owner-result")} }
                dd { "{metadata.owner_result_code}" }
                dt { {t!("admin-mail-reason-code")} }
                dd { "{metadata.reason_code}" }
                dt { {t!("admin-mail-warnings")} }
                dd { "{warnings}" }
                dt { {t!("admin-mail-delivery-ids")} }
                dd { "{delivery_ids}" }
            }
            super::deliveries::MailDeliveriesPanel {
                key: "{detail.id}",
                intake_id: detail.id.clone(),
                deliveries: detail.deliveries,
                notice: delivery_notice,
                refresh,
            }
            section { class: "admin-mail-original", aria_labelledby: "admin-mail-original-title",
                h3 { id: "admin-mail-original-title", {t!("admin-mail-original")} }
                if let Some(original) = original {
                    dl { class: "admin-mail-original__grid",
                        dt { {t!("admin-mail-original-body")} }
                        dd { pre { class: "admin-mail-original__body", "{original.body}" } }
                        dt { {t!("admin-mail-original-attachments")} }
                        dd {
                            if original.attachment_ids.is_empty() {
                                {t!("value-empty")}
                            } else {
                                ul { class: "admin-mail-original__attachments",
                                    for attachment_id in original.attachment_ids {
                                        li { "{attachment_id}" }
                                    }
                                }
                            }
                        }
                        dt { {t!("admin-mail-original-credits")} }
                        dd { "{original.credits}" }
                        dt { {t!("admin-mail-original-cod")} }
                        dd { "{original.cod}" }
                    }
                    p { class: "admin-mail-original__note", {t!("admin-mail-original-redacted")} }
                } else {
                    p { class: "admin-mail-original__missing", {t!("admin-mail-original-not-retained")} }
                }
            }
            section { class: "admin-mail-equipment-diagnostic", aria_labelledby: "admin-mail-equipment-diagnostic-title",
                h3 { id: "admin-mail-equipment-diagnostic-title", {t!("admin-mail-equipment-diagnostic")} }
                if let Some(diagnostic) = equipment_diagnostic {
                    dl { class: "admin-mail-equipment-diagnostic__grid",
                        dt { {t!("admin-mail-equipment-id")} }
                        dd { "{diagnostic.equipment_id}" }
                        dt { {t!("admin-mail-attachment-position")} }
                        dd { "{diagnostic.attachment_position}" }
                        dt { {t!("admin-mail-diagnostic-stage")} }
                        dd { "{diagnostic_stage}" }
                        dt { {t!("admin-mail-diagnostic-category")} }
                        dd { "{diagnostic.category}" }
                        dt { {t!("admin-mail-diagnostic-detail")} }
                        dd { "{diagnostic_detail}" }
                        dt { {t!("admin-mail-diagnostic-http-status")} }
                        dd { "{diagnostic_http_status}" }
                        dt { {t!("admin-mail-diagnostic-elapsed")} }
                        dd { "{diagnostic_elapsed}" }
                        dt { {t!("admin-mail-diagnostic-evidence-id")} }
                        dd { "{diagnostic_evidence_id}" }
                        dt { {t!("admin-mail-diagnostic-evidence-status")} }
                        dd { "{diagnostic_evidence_status}" }
                        dt { {t!("admin-mail-diagnostic-retention-error")} }
                        dd { "{diagnostic_retention_error}" }
                    }
                } else {
                    p { class: "admin-mail-equipment-diagnostic__missing", {t!("admin-mail-equipment-diagnostic-not-retained")} }
                }
            }
        }
    }
}

fn equipment_diagnostic_stage_label(value: &str) -> String {
    match value {
        "request" => t!("admin-mail-diagnostic-stage-request"),
        "response" => t!("admin-mail-diagnostic-stage-response"),
        "parse" => t!("admin-mail-diagnostic-stage-parse"),
        "validate" => t!("admin-mail-diagnostic-stage-validate"),
        _ => value.to_owned(),
    }
}

fn equipment_evidence_status_label(value: &str) -> String {
    match value {
        "retained" => t!("admin-mail-diagnostic-evidence-retained"),
        "body_unavailable" => t!("admin-mail-diagnostic-evidence-body-unavailable"),
        "save_failed" => t!("admin-mail-diagnostic-evidence-save-failed"),
        "completion_unconfirmed" => t!("admin-mail-diagnostic-evidence-completion-unconfirmed"),
        "prepare_failed" => t!("admin-mail-diagnostic-evidence-prepare-failed"),
        "disabled" => t!("admin-mail-diagnostic-evidence-disabled"),
        _ => value.to_owned(),
    }
}
