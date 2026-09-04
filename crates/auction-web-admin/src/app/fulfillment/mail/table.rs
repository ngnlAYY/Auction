//! Admin 邮件入站摘要表格和行展示。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, Time, TruncatedText};

use crate::api::{AdminMailIntake, AdminMailIntakePage};

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
                caption { class: "admin-visually-hidden", {t!("admin-mail-intake-title")} }
                thead {
                    tr {
                        th { scope: "col", class: "admin-mail-col-id", {t!("admin-mail-external-id")} }
                        th { scope: "col", class: "admin-mail-col-world", {t!("admin-mail-intake-world")} }
                        th { scope: "col", class: "admin-mail-col-from", {t!("admin-mail-intake-from")} }
                        th { scope: "col", class: "admin-mail-col-subject", {t!("admin-mail-intake-subject")} }
                        th { scope: "col", class: "admin-mail-col-kind", {t!("admin-mail-intake-kind")} }
                        th { scope: "col", class: "admin-mail-col-status", {t!("admin-mail-intake-status")} }
                        th { scope: "col", class: "admin-mail-col-error", {t!("admin-mail-intake-error")} }
                        th { scope: "col", {t!("admin-mail-intake-received")} }
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
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-mail-empty"),
                detail: "",
            }
        }
        Pager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("page-mails-title"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
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
                Time { value: intake.received_at.clone() }
            }
        }
    }
}
