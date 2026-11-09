//! Admin 邮件入站摘要表格和行展示。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::app::surface::pagination::{AdminPager as Pager, AdminPagination};
use auction_web_shared::{AsyncState, AsyncStateKind, Time, TruncatedText};

use crate::api::{AdminMailIntake, AdminMailIntakePage};

use super::super::sort::{SortDirection, SortableTh, sort_rows};
use super::helpers::{mail_kind_label, mail_status_class, mail_status_label};
use super::messages::processing_code_message;

#[component]
pub(super) fn MailIntakeTable(
    page: AdminMailIntakePage,
    pagination: AdminPagination,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    /// 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。
    on_page_change: EventHandler<i64>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    on_select: EventHandler<String>,
) -> Element {
    let is_empty = page.intakes.is_empty();
    let mut intakes = page.intakes;
    sort_mail_intakes(&mut intakes, &sort, direction);
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table admin-mail-table",
                caption { class: "admin-visually-hidden", {t!("admin-mail-intake-title")} }
                // 列序：邮件 ID、世界、发件人、标题、类型、状态、错误、原始信息、时间；比例总和为 100%。
                auction_web_shared::TableColumns { layout: "admin-mails", widths: vec![7, 6, 13, 19, 8, 9, 15, 12, 11] }
                thead {
                    tr {
                        th { scope: "col", class: "admin-mail-col-id", {t!("admin-mail-external-id")} }
                        SortableTh {
                            column: "world",
                            label: t!("admin-mail-intake-world"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-mail-col-world",
                        }
                        th { scope: "col", class: "admin-mail-col-from", {t!("admin-mail-intake-from")} }
                        th { scope: "col", class: "admin-mail-col-subject", {t!("admin-mail-intake-subject")} }
                        SortableTh {
                            column: "kind",
                            label: t!("admin-mail-intake-kind"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-mail-col-kind",
                        }
                        SortableTh {
                            column: "status",
                            label: t!("admin-mail-intake-status"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-mail-col-status",
                        }
                        th { scope: "col", class: "admin-mail-col-error", {t!("admin-mail-intake-error")} }
                        th { scope: "col", class: "admin-mail-col-original", {t!("admin-mail-original")} }
                        SortableTh {
                            column: "received",
                            label: t!("admin-mail-intake-received"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                    }
                }
                tbody {
                    for intake in intakes {
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
            pagination,
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("page-mails-title"),
            on_page_change,
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
    } else {
        processing_code_message(&intake.decision_code)
    };
    let sender = intake.sender.clone().unwrap_or_else(|| t!("admin-mail-original-not-retained"));
    let subject = intake.subject.clone().unwrap_or_else(|| t!("admin-mail-original-not-retained"));
    let original = if intake.has_message_display {
        t!("admin-mail-original-available")
    } else {
        t!("admin-mail-original-not-retained")
    };
    rsx! {
        tr { class: "admin-mail__row",
            td { "data-label": t!("admin-mail-external-id"),
                button {
                    class: "admin-mail__detail-trigger",
                    r#type: "button",
                    title: detail_label.clone(),
                    aria_label: detail_label,
                    onclick: move |_| on_select.call(intake_id.clone()),
                    span { class: "shared-truncated-text shared-truncated-text--compact", "{external_mail_id}" }
                }
            }
            td { "data-label": t!("admin-mail-intake-world"), TruncatedText { value: intake.world_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-from"), TruncatedText { value: sender, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-subject"), TruncatedText { value: subject, size: "wide" } }
            td { "data-label": t!("admin-mail-intake-kind"), TruncatedText { value: mail_kind_label(&intake.kind) } }
            td { "data-label": t!("admin-mail-intake-status"),
                span { class: mail_status_class(&intake.status),
                    TruncatedText { value: mail_status_label(&intake.status), size: "compact" }
                }
            }
            td { "data-label": t!("admin-mail-intake-error"),
                TruncatedText { value: decision, size: "wide" }
            }
            td { "data-label": t!("admin-mail-original"), TruncatedText { value: original, size: "compact" } }
            td {
                "data-label": t!("admin-mail-intake-received"),
                Time { value: intake.received_at.clone() }
            }
        }
    }
}

/// 对当前页入站邮件行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_mail_intakes(intakes: &mut [AdminMailIntake], column: &str, direction: SortDirection) {
    sort_rows(intakes, column, direction, |left, right, column| match column {
        "received" => left.received_at.cmp(&right.received_at),
        "kind" => left.kind.cmp(&right.kind),
        "status" => left.status.cmp(&right.status),
        "world" => left.world_id.cmp(&right.world_id),
        _ => Ordering::Equal,
    });
}
