//! Delivery 列表、行动作和分页组件。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{
    AsyncState, AsyncStateKind, Pager, TruncatedText, format_credit_amount as format_amount,
};

use crate::api::{Delivery, DeliveryPage};

use super::super::sort::{SortDirection, SortableTh, directed};
use super::dialog::DeliveryDialog;
use super::helpers::{
    delivery_display_id, delivery_display_type, delivery_status_label, is_mail_disposition_conflict,
};
use super::model::{
    DeliveryActionRequest, DeliveryDialogKind, DeliveryDialogState, DeliveryReconcileDraft,
};

/// 展示后端返回的 delivery 安全投影。
#[component]
pub(super) fn DeliveryTable(
    page: DeliveryPage,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_sort_change: EventHandler<(String, SortDirection)>,
    on_open: EventHandler<(Delivery, DeliveryDialogKind)>,
    busy_delivery: Signal<Option<String>>,
    pending_dialog: Option<DeliveryDialogState>,
    draft: Option<DeliveryReconcileDraft>,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    // 服务端不支持排序参数，浏览器只对当前页排序；未选择排序列时保持原始顺序。
    let mut deliveries = page.deliveries.clone();
    sort_deliveries(&mut deliveries, &sort, direction);
    rsx! {
        div { class: "admin-delivery-table-wrap",
            table { class: "admin-delivery-table",
                caption { class: "admin-visually-hidden", {t!("page-deliveries-title")} }
                // 列序：ID、类型、收件人、COD、状态、次数、错误、操作；按字段预期内容分配比例。
                auction_web_shared::TableColumns { layout: "admin-deliveries", widths: vec![8, 10, 15, 12, 9, 8, 22, 16] }
                thead {
                    tr {
                        SortableTh {
                            column: "id",
                            label: t!("delivery-id"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-id",
                        }
                        th { scope: "col", class: "admin-delivery-col-type", {t!("delivery-type")} }
                        th { scope: "col", class: "admin-delivery-col-target", {t!("delivery-to")} }
                        SortableTh {
                            column: "cod",
                            label: t!("delivery-cod"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-cod",
                        }
                        SortableTh {
                            column: "status",
                            label: t!("delivery-status"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-status",
                        }
                        SortableTh {
                            column: "attempts",
                            label: t!("delivery-attempts"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-attempts",
                        }
                        th { scope: "col", class: "admin-delivery-col-error", {t!("delivery-last-error")} }
                        th { scope: "col", {t!("catalog-actions")} }
                    }
                }
                tbody {
                    for delivery in deliveries {
                        // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                        DeliveryRow {
                            key: "{delivery.id}",
                            delivery,
                            on_open,
                            busy_delivery,
                            pending_dialog: pending_dialog.clone(),
                            draft: draft.clone(),
                            on_draft_change,
                            on_confirm,
                            on_cancel,
                        }
                    }
                }
            }
        }
        if page.deliveries.is_empty() {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("delivery-empty"),
                detail: "",
            }
        }
    }
}

/// 为单条 delivery 选择服务端允许的动作。
#[component]
pub(super) fn DeliveryRow(
    delivery: Delivery,
    on_open: EventHandler<(Delivery, DeliveryDialogKind)>,
    busy_delivery: Signal<Option<String>>,
    pending_dialog: Option<DeliveryDialogState>,
    draft: Option<DeliveryReconcileDraft>,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let kind = match delivery.status.as_str() {
        "claimed" => Some(DeliveryDialogKind::Release),
        "failed" if !is_mail_disposition_conflict(&delivery) => Some(DeliveryDialogKind::Retry),
        "manual_reconcile" => Some(DeliveryDialogKind::Reconcile),
        _ => None,
    };
    let action_label = kind.map(DeliveryDialogKind::action_label);
    let status = delivery_status_label(&delivery.status);
    let cod = delivery.cod.map(format_amount).unwrap_or_else(|| t!("value-empty"));
    let failure = delivery.failure_code.clone().unwrap_or_default();
    let recipient = delivery.recipient.clone();
    let hidden_recipient = recipient.clone();
    let full_identifier = if delivery.origin_id.is_empty() {
        delivery.id.clone()
    } else {
        delivery.origin_id.clone()
    };
    let display_identifier = delivery_display_id(&delivery);
    let display_type = delivery_display_type(&delivery.origin_kind);
    let delivery_for_action = delivery.clone();

    rsx! {
        tr {
            td {
                TruncatedText {
                    value: full_identifier,
                    display: display_identifier,
                    size: "compact",
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                TruncatedText {
                    value: display_type,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                h3 { class: "admin-visually-hidden", "{hidden_recipient}" }
                TruncatedText {
                    value: recipient,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "admin-delivery-num",
                "{cod}"
            }
            td {
                span { class: "admin-delivery-badge", {status} }
            }
            td {
                class: "admin-delivery-num",
                "{delivery.attempt_count}"
            }
            td {
                TruncatedText {
                    value: failure,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                div { class: "admin-delivery-actions",
                    if let (Some(kind), Some(label)) = (kind, action_label) {
                        button {
                            r#type: "button",
                            class: "admin-btn-primary",
                            disabled: busy_delivery().is_some(),
                            onclick: move |_| on_open.call((delivery_for_action.clone(), kind)),
                            "{label}"
                        }
                    }
                }
            }
        }
        if let Some(dialog) = pending_dialog.filter(|dialog| {
            dialog.kind == DeliveryDialogKind::Reconcile && dialog.delivery.id == delivery.id
        }) {
            tr {
                class: "admin-delivery-dialog-row",
                td {
                    colspan: "8",
                    DeliveryDialog {
                        delivery: dialog.delivery,
                        kind: dialog.kind,
                        busy: busy_delivery().is_some(),
                        draft: draft.unwrap_or_else(|| {
                            DeliveryReconcileDraft::for_delivery(&delivery)
                        }),
                        on_draft_change,
                        on_confirm,
                        on_cancel,
                    }
                }
            }
        }
    }
}

/// 控制 delivery 列表分页。
#[component]
pub(super) fn DeliveryPager(page: DeliveryPage, on_page_change: EventHandler<i64>) -> Element {
    rsx! {
        Pager {
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("delivery-pagination"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_page_change,
        }
    }
}

/// 对当前页 delivery 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
/// 到付金额缺省（None）在升序时排在最前。
fn sort_deliveries(deliveries: &mut [Delivery], column: &str, direction: SortDirection) {
    if column.is_empty() {
        return;
    }
    deliveries.sort_by(|left, right| {
        directed(
            match column {
                "id" => left.id.cmp(&right.id),
                "cod" => left.cod.cmp(&right.cod),
                "status" => left.status.cmp(&right.status),
                "attempts" => left.attempt_count.cmp(&right.attempt_count),
                _ => Ordering::Equal,
            },
            direction,
        )
    });
}
