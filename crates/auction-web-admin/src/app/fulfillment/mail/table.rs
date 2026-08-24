//! Admin 邮件入站摘要表格和行展示。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::TruncatedText;

use crate::api::{AdminMailIntake, AdminMailIntakePage};

use super::super::format_admin_time;
use super::super::pagination::AdminPager;
use super::helpers::{mail_decision_class, mail_kind_label, mail_status_class, mail_status_label};

#[component]
pub(super) fn MailIntakeTable(
    page: AdminMailIntakePage,
    mut page_number: Signal<i64>,
    on_select: EventHandler<String>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-mail-intake-title",
            h2 { id: "admin-mail-intake-title", {t!("admin-mail-intake-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-intake-total", total: page.pagination.total)} }
            if page.intakes.is_empty() {
                p { class: "admin-panel__note", {t!("admin-mail-empty")} }
            } else {
                div { class: "public-auction-catalog admin-mail__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-mail-external-id")} }
                                th { {t!("admin-mail-intake-world")} }
                                th { {t!("admin-mail-intake-kind")} }
                                th { {t!("admin-mail-intake-status")} }
                                th { {t!("admin-mail-intake-decision")} }
                                th { {t!("admin-mail-intake-received")} }
                                th { {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for intake in page.intakes {
                                MailIntakeRow { intake, on_select }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("admin-mail-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}

#[component]
fn MailIntakeRow(intake: AdminMailIntake, on_select: EventHandler<String>) -> Element {
    let intake_id = intake.id.clone();
    rsx! {
        tr { class: "admin-mail__row",
            td { "data-label": t!("admin-mail-external-id"), TruncatedText { value: intake.external_mail_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-world"), TruncatedText { value: intake.world_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-kind"), TruncatedText { value: mail_kind_label(&intake.kind) } }
            td { "data-label": t!("admin-mail-intake-status"),
                span { class: mail_status_class(&intake.status), TruncatedText { value: mail_status_label(&intake.status), size: "compact" } }
            }
            td { "data-label": t!("admin-mail-intake-decision"),
                span { class: mail_decision_class(&intake.decision_code), "{intake.decision_code}" }
            }
            td {
                "data-label": t!("admin-mail-intake-received"),
                time { datetime: intake.received_at.clone(), "{format_admin_time(&intake.received_at)}" }
            }
            td { "data-label": t!("catalog-actions"),
                button {
                    r#type: "button",
                    onclick: move |_| on_select.call(intake_id.clone()),
                    {t!("admin-mail-open-detail")}
                }
            }
        }
    }
}
