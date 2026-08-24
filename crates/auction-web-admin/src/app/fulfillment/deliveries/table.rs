//! Delivery 列表、行动作和分页组件。

use dioxus::prelude::*;
use dioxus_i18n::t;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{Delivery, DeliveryPage};
use crate::app::surface::pagination::AdminPager;

use super::helpers::{delivery_origin_label, delivery_status_class, delivery_status_label};
use super::model::DeliveryDialogKind;

/// 展示后端返回的 delivery 安全投影。
#[component]
pub(super) fn DeliveryTable(
    page: DeliveryPage,
    on_open: EventHandler<(Delivery, DeliveryDialogKind)>,
    busy_delivery: Signal<Option<String>>,
) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            aria_labelledby: "delivery-table-title",
            aria_busy: busy_delivery().is_some().to_string(),
            h2 { id: "delivery-table-title", {t!("delivery-total", total: page.total)} }
            if page.deliveries.is_empty() {
                p { class: "admin-panel__note", {t!("delivery-empty")} }
            } else {
                div { class: "public-auction-catalog admin-delivery__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("delivery-id")} }
                                th { {t!("delivery-recipient")} }
                                th { {t!("delivery-cod")} }
                                th { {t!("delivery-origin")} }
                                th { {t!("delivery-status")} }
                                th { {t!("delivery-attempts")} }
                                th { {t!("delivery-fencing")} }
                                th { {t!("delivery-failure")} }
                                th { {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for delivery in page.deliveries {
                                DeliveryRow { delivery, on_open, busy_delivery }
                            }
                        }
                    }
                }
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
) -> Element {
    let kind = match delivery.status.as_str() {
        "claimed" => Some(DeliveryDialogKind::Release),
        "failed" => Some(DeliveryDialogKind::Retry),
        "manual_reconcile" => Some(DeliveryDialogKind::Reconcile),
        _ => None,
    };
    let action_label = kind.map(DeliveryDialogKind::action_label);
    let is_busy = busy_delivery().as_deref() == Some(delivery.id.as_str());
    let status = delivery_status_label(&delivery.status);
    let cod = delivery.cod.map(format_amount).unwrap_or_else(|| t!("value-empty"));
    let failure = delivery.failure_code.clone().unwrap_or_else(|| t!("value-empty"));
    let recipient = delivery.recipient.clone();
    let origin =
        format!("{}: {}", delivery_origin_label(&delivery.origin_kind), delivery.origin_id);
    let delivery_for_action = delivery.clone();

    rsx! {
        tr { class: "admin-delivery__row",
            td { "data-label": t!("delivery-id"), TruncatedText { value: delivery.id, size: "wide" } }
            td { "data-label": t!("delivery-recipient"), TruncatedText { value: recipient } }
            td {
                "data-label": t!("delivery-cod"),
                class: "public-auction-catalog__amount public-auction-catalog__number",
                "{cod}"
            }
            td {
                "data-label": t!("delivery-origin"),
                TruncatedText { value: origin }
            }
            td { "data-label": t!("delivery-status"),
                span {
                    class: delivery_status_class(&delivery.status),
                    {status}
                }
            }
            td {
                "data-label": t!("delivery-attempts"),
                class: "public-auction-catalog__number",
                "{delivery.attempt_count}"
            }
            td {
                "data-label": t!("delivery-fencing"),
                class: "public-auction-catalog__number",
                "{delivery.fencing_version}"
            }
            td { "data-label": t!("delivery-failure"), TruncatedText { value: failure } }
            td { "data-label": t!("catalog-actions"), class: "admin-delivery__actions",
                if let (Some(kind), Some(label)) = (kind, action_label) {
                    button {
                        r#type: "button",
                        class: "admin-btn-primary",
                        disabled: busy_delivery().is_some(),
                        aria_busy: is_busy.to_string(),
                        onclick: move |_| on_open.call((delivery_for_action.clone(), kind)),
                        if is_busy { {t!("admin-action-working")} } else { "{label}" }
                    }
                } else {
                    span { {t!("delivery-no-action")} }
                }
            }
        }
    }
}

/// 控制 delivery 列表分页。
#[component]
pub(super) fn DeliveryPager(page: DeliveryPage, mut page_number: Signal<i64>) -> Element {
    rsx! {
        AdminPager {
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            label: t!("delivery-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
