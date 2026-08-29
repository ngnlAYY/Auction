//! Admin 邮件入站摘要表格和行展示。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::TruncatedText;

use crate::api::{AdminMailIntake, AdminMailIntakePage};

use super::super::format_admin_time;
use super::super::pagination::AdminPager;

#[component]
pub(super) fn MailIntakeTable(
    page: AdminMailIntakePage,
    mut page_number: Signal<i64>,
    on_select: EventHandler<String>,
) -> Element {
    let is_empty = page.intakes.is_empty();
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-mail-table",
                thead {
                    tr {
                        th { class: "admin-mail-col-id", {t!("admin-mail-external-id")} }
                        th { class: "admin-mail-col-world", {t!("admin-mail-intake-world")} }
                        th { class: "admin-mail-col-from", {t!("admin-mail-intake-from")} }
                        th { class: "admin-mail-col-subject", {t!("admin-mail-intake-subject")} }
                        th { class: "admin-mail-col-kind", {t!("admin-mail-intake-kind")} }
                        th { class: "admin-mail-col-status", {t!("admin-mail-intake-status")} }
                        th { class: "admin-mail-col-error", {t!("admin-mail-intake-error")} }
                        th { {t!("admin-mail-intake-received")} }
                    }
                }
                tbody {
                    for intake in page.intakes {
                        MailIntakeRow { intake, on_select }
                    }
                }
            }
        }
        if is_empty {
            p { class: "admin-shell__status", {t!("admin-mail-empty")} }
        }
        AdminPager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: page.pagination.total,
            label: t!("page-mails-title"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}

#[component]
fn MailIntakeRow(intake: AdminMailIntake, on_select: EventHandler<String>) -> Element {
    let intake_id = intake.id.clone();
    let external_mail_id = intake.external_mail_id.clone();
    let detail_label = format!("{}: {external_mail_id}", t!("admin-mail-open-detail"));
    let decision = if intake.status == "accepted" {
        String::new()
    } else if intake.decision_code.is_empty() {
        t!("value-empty")
    } else {
        intake.decision_code.clone()
    };
    rsx! {
        tr { class: "admin-mail__row",
            td { "data-label": t!("admin-mail-external-id"),
                button {
                    class: "admin-mail__detail-trigger",
                    r#type: "button",
                    title: t!("admin-mail-open-detail"),
                    aria_label: detail_label,
                    onclick: move |_| on_select.call(intake_id.clone()),
                    TruncatedText { value: external_mail_id, size: "compact" }
                }
            }
            td { "data-label": t!("admin-mail-intake-world"), TruncatedText { value: intake.world_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-from"), {t!("value-empty")} }
            td { "data-label": t!("admin-mail-intake-subject"), {t!("value-empty")} }
            td { "data-label": t!("admin-mail-intake-kind"), TruncatedText { value: intake.kind } }
            td { "data-label": t!("admin-mail-intake-status"),
                span { class: "admin-mail-badge", TruncatedText { value: intake.status, size: "compact" } }
            }
            td { "data-label": t!("admin-mail-intake-error"),
                TruncatedText { value: decision, size: "wide" }
            }
            td {
                "data-label": t!("admin-mail-intake-received"),
                time { datetime: intake.received_at.clone(), "{format_admin_time(&intake.received_at)}" }
            }
        }
    }
}
