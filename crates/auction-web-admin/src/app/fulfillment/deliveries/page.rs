//! Delivery 页面状态、查询资源和动作编排。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminApiFailure, Delivery, DeliveryAction, execute_delivery, fetch_deliveries};
use crate::app::surface::{AdminActionFeedback, admin_api_error_message};

use super::dialog::DeliveryDialog;
use super::filters::DeliveryFilters;
use super::model::{
    DeliveryActionRequest, DeliveryDialogKind, DeliveryDialogState, DeliveryReconcileDraft,
};
use super::table::{DeliveryPager, DeliveryTable};

/// 加载 delivery 安全投影并编排带 fencing 的操作员动作。
pub fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut success_action = use_signal(|| None::<DeliveryAction>);
    let mut busy_delivery = use_signal(|| None::<String>);
    let status_filter = use_signal(String::new);
    let page_number = use_signal(|| 1_i64);
    let mut pending_dialog = use_signal(|| None::<DeliveryDialogState>);
    let mut reconcile_draft = use_signal(|| None::<DeliveryReconcileDraft>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let deliveries = use_resource(move || {
        let _ = refresh();
        fetch_deliveries(status_filter(), page_number())
    });

    let open_dialog = move |(delivery, kind): (Delivery, DeliveryDialogKind)| {
        if busy_delivery().is_none() {
            notice.set(None);
            success_action.set(None);
            if kind == DeliveryDialogKind::Reconcile
                && reconcile_draft().as_ref().is_none_or(|draft| draft.delivery_id != delivery.id)
            {
                reconcile_draft
                    .set(Some(DeliveryReconcileDraft::for_delivery(delivery.id.clone())));
            }
            pending_dialog.set(Some(DeliveryDialogState { delivery, kind }));
        }
    };
    let update_reconcile_draft = move |draft: DeliveryReconcileDraft| {
        if reconcile_draft()
            .as_ref()
            .is_some_and(|current| current.delivery_id == draft.delivery_id)
        {
            reconcile_draft.set(Some(draft));
        }
    };
    let confirm_action = move |request: DeliveryActionRequest| {
        if busy_delivery().is_some() {
            return;
        }
        if request.action == DeliveryAction::ReconcileRetry
            && let Some(dialog) = pending_dialog()
            && dialog.kind == DeliveryDialogKind::Reconcile
        {
            // 旧版 retry 人工对账先关闭字段框，再进入共用的第二确认框；此时不能发 mutation。
            pending_dialog.set(Some(DeliveryDialogState {
                delivery: dialog.delivery,
                kind: DeliveryDialogKind::ReconcileRetry,
            }));
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
        success_action.set(None);
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
        let mut success_action = success_action;
        spawn(async move {
            let result = execute_delivery(
                command_id,
                delivery_id,
                action,
                expected_fencing_version,
                external_reference,
                failure_code,
            )
            .await;
            busy_delivery.set(None);
            match result {
                Ok(_) => {
                    notice.set(Some(AdminActionFeedback::Success));
                    success_action.set(Some(action));
                    pending_dialog.set(None);
                    command_state.set(None);
                    reconcile_draft.set(None);
                    refresh.set(refresh().saturating_add(1));
                }
                Err(error) if is_stale_delivery(&error) => {
                    // 版本已失效时不能继续保留旧 fencing 和命令上下文，必须回到权威列表。
                    pending_dialog.set(None);
                    command_state.set(None);
                    reconcile_draft.set(None);
                    notice.set(None);
                    success_action.set(None);
                    refresh.set(refresh().saturating_add(1));
                }
                Err(error) => notice.set(Some(AdminActionFeedback::Error(error.code))),
            }
        });
    };

    let (page, load_error) = match &*deliveries.read_unchecked() {
        None => (None, None),
        Some(Err(error)) => (None, Some(admin_api_error_message(error))),
        Some(Ok(page)) => (Some(page.clone()), None),
    };
    let pending = pending_dialog();
    let draft = reconcile_draft();
    let row_dialog = pending.clone().filter(|dialog| dialog.kind == DeliveryDialogKind::Reconcile);
    let page_dialog = pending.filter(|dialog| dialog.kind != DeliveryDialogKind::Reconcile);
    let success_message = delivery_success_message(success_action());

    rsx! {
        section { class: "admin-delivery-page", aria_labelledby: "delivery-title",
            h1 { id: "delivery-title", {t!("page-deliveries-title")} }
            DeliveryFilters { status_filter, page_number }
            if let Some(page) = page.clone() {
                DeliveryTable {
                    page,
                    on_open: open_dialog,
                    busy_delivery,
                    pending_dialog: row_dialog,
                    draft: draft.clone(),
                    on_draft_change: update_reconcile_draft,
                    on_confirm: confirm_action,
                    on_cancel: move |_| {
                        pending_dialog.set(None);
                        reconcile_draft.set(None);
                        command_state.set(None);
                    },
                }
            } else if let Some(error) = load_error {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: error,
                    detail: "",
                }
            } else {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("data-loading"),
                    detail: "",
                }
            }
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "admin-delivery-status"
                    } else {
                        "admin-delivery-error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(success_message)}
                }
            }
            if let Some(page) = page {
                DeliveryPager { page, page_number }
            }
            if let Some(dialog) = page_dialog {
                DeliveryDialog {
                    delivery: dialog.delivery.clone(),
                    kind: dialog.kind,
                    busy: busy_delivery().is_some(),
                    draft: draft.clone().unwrap_or_else(|| {
                        DeliveryReconcileDraft::for_delivery(dialog.delivery.id.clone())
                    }),
                    on_draft_change: move |_| {},
                    on_confirm: confirm_action,
                    on_cancel: move |_| {
                        pending_dialog.set(None);
                        reconcile_draft.set(None);
                        command_state.set(None);
                    },
                }
            }
        }
    }
}

fn is_stale_delivery(error: &AdminApiFailure) -> bool {
    error.is_conflict()
        && matches!(error.code.as_str(), "stale_delivery_state" | "stale_state_version")
}

fn delivery_success_message(action: Option<DeliveryAction>) -> String {
    match action {
        Some(DeliveryAction::Release) => t!("delivery-action-released"),
        Some(DeliveryAction::Retry) => t!("delivery-action-retried"),
        Some(
            DeliveryAction::ReconcileRetry
            | DeliveryAction::ReconcileSucceeded
            | DeliveryAction::ReconcileFailed,
        ) => t!("delivery-action-reconciled"),
        None => t!("delivery-action-success"),
    }
}
