//! Admin 邮件入站详情安全投影。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::AdminMailIntakeDetail;

use super::super::format_admin_time;
use super::helpers::{mail_kind_label, mail_status_class, mail_status_label};

#[component]
pub(super) fn MailIntakeDetailPanel(
    visible: bool,
    detail: Option<AdminMailIntakeDetail>,
    loading: bool,
    error: bool,
    mut refresh: Signal<u64>,
    on_close: EventHandler<()>,
) -> Element {
    if !visible {
        return rsx! {};
    }

    if loading {
        return rsx! {
            section { class: "admin-panel admin-mail-detail", aria_live: "polite",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-detail-loading")} }
            }
        };
    }

    if error {
        return rsx! {
            section { class: "admin-panel admin-mail-detail", role: "alert",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-detail-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| refresh.set(refresh().saturating_add(1)),
                    {t!("admin-retry")}
                }
                button { r#type: "button", onclick: move |_| on_close.call(()), {t!("admin-mail-close-detail")} }
            }
        };
    }

    let Some(detail) = detail else {
        return rsx! {
            section { class: "admin-panel admin-mail-detail",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-no-selection")} }
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
        }
    }
}
