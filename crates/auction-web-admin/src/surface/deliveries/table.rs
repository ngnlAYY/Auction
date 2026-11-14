//! Delivery 列表、行动作和分页组件。

use std::cmp::Ordering;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{
    AsyncState, AsyncStateKind, TruncatedText, format_credit_amount as format_amount,
};

use crate::api::{Delivery, DeliveryPage};
use crate::surface::pending::PendingCommands;

use super::super::Route;
use super::super::sort::{MobileSortControl, SortDirection, SortableTh, sort_rows};
use super::dialog::DeliveryDialog;
use super::helpers::{
    delivery_category_label, delivery_display_id, delivery_display_type, delivery_outcome_label,
    delivery_status_label, is_mail_disposition_conflict,
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
    pending_dialog: Option<DeliveryDialogState>,
    draft: Option<DeliveryReconcileDraft>,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let is_empty = page.deliveries.is_empty();
    let mut deliveries = page.deliveries;
    sort_deliveries(&mut deliveries, &sort, direction);
    rsx! {
        MobileSortControl {
            columns: vec![
                ("id", t!("delivery-id")),
                ("template", t!("delivery-template")),
                ("category", t!("delivery-category")),
                ("cod", t!("delivery-cod")),
                ("status", t!("delivery-status")),
                ("attempts", t!("delivery-attempts")),
            ],
            sort: sort.clone(),
            direction,
            on_sort_change,
        }
        div { class: "admin-delivery-table-wrap",
            table { class: "admin-delivery-table",
                aria_label: t!("page-deliveries-title"),
                auction_web_shared::TableColumns { layout: "admin-deliveries", widths: vec![7, 9, 13, 11, 9, 7, 7, 6, 21, 10] }
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
                            column: "template",
                            label: t!("delivery-template"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-template",
                        }
                        SortableTh {
                            column: "category",
                            label: t!("delivery-category"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-category",
                        }
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
                            active: sort,
                            direction,
                            on_sort: on_sort_change,
                            class: "admin-delivery-col-attempts",
                        }
                        th { scope: "col", class: "admin-delivery-col-error", {t!("delivery-outcome")} }
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
        if is_empty {
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
    pending_dialog: Option<DeliveryDialogState>,
    draft: Option<DeliveryReconcileDraft>,
    on_draft_change: EventHandler<DeliveryReconcileDraft>,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let commands = use_context::<PendingCommands>();
    let kind = match delivery.status.as_str() {
        "claimed" => Some(DeliveryDialogKind::Release),
        "failed"
            if !is_mail_disposition_conflict(&delivery)
                && !matches!(
                    delivery.reason_code.as_deref(),
                    Some("lot_watch_invalidated" | "recipient_renamed")
                ) =>
        {
            Some(DeliveryDialogKind::Retry)
        }
        "manual_reconcile" => Some(DeliveryDialogKind::Reconcile),
        _ => None,
    };
    let action_label = kind.map(DeliveryDialogKind::action_label);
    let status = delivery_status_label(&delivery.status);
    let cod = delivery.cod.map_or_else(|| t!("value-empty"), format_amount);
    let template = delivery.template.clone().unwrap_or_else(|| t!("value-empty"));
    let category =
        delivery.category.as_deref().map_or_else(|| t!("value-empty"), delivery_category_label);
    let outcome = delivery_outcome_label(&delivery);
    let recipient = delivery.recipient.clone();
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
                "data-label": t!("delivery-id"),
                TruncatedText {
                    value: full_identifier,
                    display: display_identifier,
                    size: "compact",
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                "data-label": t!("delivery-type"),
                TruncatedText {
                    value: display_type,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                "data-label": t!("delivery-to"),
                TruncatedText {
                    value: recipient,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                "data-label": t!("delivery-template"),
                TruncatedText {
                    value: template,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                "data-label": t!("delivery-category"),
                TruncatedText {
                    value: category,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "admin-delivery-num",
                "data-label": t!("delivery-cod"),
                "{cod}"
            }
            td {
                "data-label": t!("delivery-status"),
                span { class: "admin-delivery-badge", {status} }
            }
            td {
                class: "admin-delivery-num",
                "data-label": t!("delivery-attempts"),
                "{delivery.attempt_count}"
            }
            td {
                "data-label": t!("delivery-outcome"),
                TruncatedText {
                    value: outcome,
                    portal: true,
                    legacy_dom: true,
                }
            }
            td {
                class: "admin-delivery-actions-cell",
                div { class: "admin-delivery-actions",
                    Link {
                        class: "admin-delivery-detail-link",
                        to: Route::DeliveryDetail { delivery_id: delivery.id.clone() },
                        {t!("delivery-detail-open")}
                    }
                    if let (Some(kind), Some(label)) = (kind, action_label) {
                        button {
                            r#type: "button",
                            class: "admin-btn-primary",
                            disabled: commands.any_busy() || commands.contains(&delivery.id),
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
                    colspan: "10",
                    DeliveryDialog {
                        delivery: dialog.delivery,
                        kind: dialog.kind,
                        busy: commands.any_busy(),
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
/// 对当前页 delivery 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
/// 到付金额缺省（None）在升序时排在最前。
fn sort_deliveries(deliveries: &mut [Delivery], column: &str, direction: SortDirection) {
    sort_rows(deliveries, column, direction, |left, right, column| match column {
        "id" => left.id.cmp(&right.id),
        "template" => left.template.cmp(&right.template),
        "category" => left.category.cmp(&right.category),
        "cod" => left.cod.cmp(&right.cod),
        "status" => left.status.cmp(&right.status),
        "attempts" => left.attempt_count.cmp(&right.attempt_count),
        _ => Ordering::Equal,
    });
}
