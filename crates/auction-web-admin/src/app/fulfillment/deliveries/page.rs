//! Delivery 页面状态、查询资源和动作编排。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{Delivery, execute_delivery, fetch_deliveries};
use crate::app::surface::{AdminLoadError, AdminPanel};

use super::dialog::DeliveryDialog;
use super::filters::DeliveryFilters;
use super::model::{DeliveryActionRequest, DeliveryDialogKind, DeliveryDialogState};
use super::table::{DeliveryPager, DeliveryTable};

/// 加载 delivery 安全投影并编排带 fencing 的操作员动作。
pub fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<bool>);
    let mut busy_delivery = use_signal(|| None::<String>);
    let status_filter = use_signal(String::new);
    let page_number = use_signal(|| 1_i64);
    let mut pending_dialog = use_signal(|| None::<DeliveryDialogState>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let deliveries = use_resource(move || {
        let _ = refresh();
        fetch_deliveries(status_filter(), page_number())
    });

    let open_dialog = move |(delivery, kind): (Delivery, DeliveryDialogKind)| {
        if busy_delivery().is_none() {
            notice.set(None);
            pending_dialog.set(Some(DeliveryDialogState { delivery, kind }));
        }
    };
    let confirm_action = move |request: DeliveryActionRequest| {
        if busy_delivery().is_some() {
            return;
        }
        let DeliveryActionRequest {
            delivery_id,
            action,
            expected_fencing_version,
            external_reference,
            failure_code,
        } = request;
        notice.set(None);
        busy_delivery.set(Some(delivery_id.clone()));
        let signature = format!(
            "{delivery_id}:{action:?}:{expected_fencing_version}:{external_reference:?}:{failure_code:?}"
        );
        let command_id = match command_state() {
            Some((current, command_id)) if current == signature => command_id,
            _ => {
                let command_id = Uuid::new_v4();
                command_state.set(Some((signature, command_id)));
                command_id
            }
        };
        let mut busy_delivery = busy_delivery;
        spawn(async move {
            let successful = execute_delivery(
                command_id,
                delivery_id,
                action,
                expected_fencing_version,
                external_reference,
                failure_code,
            )
            .await
            .is_ok();
            busy_delivery.set(None);
            notice.set(Some(successful));
            if successful {
                pending_dialog.set(None);
                command_state.set(None);
                refresh.set(refresh().saturating_add(1));
            }
        });
    };

    match &*deliveries.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-deliveries-title"), detail: t!("data-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-deliveries-title"),
                detail: t!("data-error"),
                refresh,
            }
        },
        Some(Ok(page)) => {
            let pending = pending_dialog();
            rsx! {
                section { class: "admin-panel", aria_labelledby: "delivery-title",
                    h1 { id: "delivery-title", {t!("page-deliveries-title")} }
                    p { class: "admin-panel__note", {t!("page-deliveries-note")} }
                    DeliveryFilters { status_filter, page_number }
                    DeliveryTable { page: page.clone(), on_open: open_dialog, busy_delivery }
                    DeliveryPager { page: page.clone(), page_number }
                }
                if let Some(dialog) = pending {
                    DeliveryDialog {
                        delivery: dialog.delivery,
                        kind: dialog.kind,
                        busy: busy_delivery().is_some(),
                        on_confirm: confirm_action,
                        on_cancel: move |_| pending_dialog.set(None),
                    }
                }
                if let Some(successful) = notice() {
                    p {
                        class: if successful {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if successful { "status" } else { "alert" },
                        if successful { {t!("delivery-action-success")} } else { {t!("delivery-action-error")} }
                    }
                }
            }
        }
    }
}
