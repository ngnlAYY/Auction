//! Admin delivery outbox 页面。
//!
//! 页面只操作后端返回的安全投影，并把快照中的 fencing version 原样回传。外部传输结果不由
//! 浏览器推断；未知结果只能显示为人工对账状态，具体证明由服务端命令端口校验。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{Delivery, DeliveryAction, DeliveryPage, execute_delivery, fetch_deliveries};

use super::AdminPanel;

pub(super) fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<bool>);
    let deliveries = use_resource(move || {
        let _ = refresh();
        fetch_deliveries()
    });
    let on_action = move |(delivery_id, action, fencing_version): (String, DeliveryAction, i64)| {
        notice.set(None);
        spawn(async move {
            let successful = execute_delivery(delivery_id, action, fencing_version).await.is_ok();
            notice.set(Some(successful));
            if successful {
                refresh.set(refresh() + 1);
            }
        });
    };

    match &*deliveries.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-deliveries-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminPanel { title: t!("page-deliveries-title"), detail: t!("data-error") }
        },
        Some(Ok(page)) => rsx! {
            DeliveryTable { page: page.clone(), on_action }
            if let Some(successful) = notice() {
                p { class: "admin-panel__note", role: "status",
                    if successful { {t!("delivery-action-success")} } else { {t!("delivery-action-error")} }
                }
            }
        },
    }
}

#[component]
fn DeliveryTable(
    page: DeliveryPage,
    on_action: EventHandler<(String, DeliveryAction, i64)>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "delivery-title",
            h1 { id: "delivery-title", {t!("page-deliveries-title")} }
            p { class: "admin-panel__note", {t!("delivery-total", total: page.total)} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("delivery-recipient")} }
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
                            DeliveryRow { delivery, on_action }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeliveryRow(
    delivery: Delivery,
    on_action: EventHandler<(String, DeliveryAction, i64)>,
) -> Element {
    let delivery_id = delivery.id.clone();
    let fencing_version = delivery.fencing_version;
    let action = match delivery.status.as_str() {
        "claimed" => Some(DeliveryAction::Release),
        "failed" => Some(DeliveryAction::Retry),
        "manual_reconcile" => Some(DeliveryAction::ReconcileRetry),
        _ => None,
    };
    let action_label = action.map(|action| match action {
        DeliveryAction::Release => t!("delivery-release"),
        DeliveryAction::Retry => t!("delivery-retry"),
        DeliveryAction::ReconcileRetry => t!("delivery-reconcile-retry"),
    });
    let status = delivery_status_label(&delivery.status);
    let failure = delivery.failure_code.unwrap_or_else(|| t!("value-empty"));

    rsx! {
        tr {
            td { "{delivery.recipient}" }
            td { "{delivery.origin_kind}: {delivery.origin_id}" }
            td { "{status}" }
            td { class: "public-auction-catalog__number", "{delivery.attempt_count}" }
            td { class: "public-auction-catalog__number", "{delivery.fencing_version}" }
            td { "{failure}" }
            td {
                if let (Some(action), Some(label)) = (action, action_label) {
                    button {
                        r#type: "button",
                        onclick: move |_| on_action.call((delivery_id.clone(), action, fencing_version)),
                        "{label}"
                    }
                } else {
                    span { {t!("delivery-no-action")} }
                }
            }
        }
    }
}

fn delivery_status_label(status: &str) -> String {
    match status {
        "prepared" => t!("delivery-status-prepared"),
        "pending" => t!("delivery-status-pending"),
        "claimed" => t!("delivery-status-claimed"),
        "sending" => t!("delivery-status-sending"),
        "succeeded" => t!("delivery-status-succeeded"),
        "failed" => t!("delivery-status-failed"),
        "manual_reconcile" => t!("delivery-status-manual"),
        _ => status.to_owned(),
    }
}
