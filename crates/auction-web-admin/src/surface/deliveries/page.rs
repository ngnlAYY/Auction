//! Delivery 页面状态、查询资源和动作编排。

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;

use super::super::query::use_deliveries_query_state;
use super::dialog::DeliveryDialog;
use super::filters::{DeliveryFilters, DeliveryFiltersDraft};
use super::labels::is_mail_disposition_conflict;
use super::model::{
    DeliveryActionRequest, DeliveryDialogKind, DeliveryDialogState, DeliveryReconcileDraft,
};
use super::table::DeliveryTable;
use crate::api::{Delivery, DeliveryAction, execute_delivery, fetch_deliveries, valid_uuid};
use crate::surface::pagination::{AdminPager, use_admin_page, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

/// 加载 delivery 安全投影并编排带 fencing 的操作员动作。
pub fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut success_action = use_signal(|| None::<DeliveryAction>);
    let commands = use_pending_commands(refresh);
    // 状态筛选与分页跟地址栏双向同步；对话框、对账草稿和命令上下文是瞬态，不入 URL。
    let query = use_deliveries_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.status.clone(), current.filters(), current.page)
    });
    let mut pending_dialog = use_signal(|| None::<DeliveryDialogState>);
    let mut reconcile_draft = use_signal(|| None::<DeliveryReconcileDraft>);
    // 玩家报出的投递 Reference 可直接定位详情；只接受规范 UUID，不猜测其他编号。
    let mut lookup = use_signal(String::new);
    let mut lookup_invalid = use_signal(|| false);
    let navigator = navigator();
    let open_lookup = move |event: FormEvent| {
        event.prevent_default();
        let target = lookup().trim().to_owned();
        if !valid_uuid(&target) {
            lookup_invalid.set(true);
            return;
        }
        lookup_invalid.set(false);
        let _ = navigator.push(format!("/deliveries/{target}"));
    };
    let pagination = use_admin_pagination("deliveries", &["page_size"], Some(100), i64::MAX);
    let deliveries = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |(status, filters, page), size| fetch_deliveries(status, filters, page, size),
    );

    let open_dialog = move |(delivery, kind): (Delivery, DeliveryDialogKind)| {
        if !commands.any_busy() && !commands.contains(&delivery.id) {
            notice.set(None);
            success_action.set(None);
            if kind == DeliveryDialogKind::Reconcile
                && reconcile_draft().as_ref().is_none_or(|draft| {
                    draft.delivery_id != delivery.id
                        || (is_mail_disposition_conflict(&delivery) && draft.action != "failed")
                })
            {
                reconcile_draft.set(Some(DeliveryReconcileDraft::for_delivery(&delivery)));
            }
            pending_dialog.set(Some(DeliveryDialogState { delivery, kind }));
        }
    };
    let update_reconcile_draft = move |draft: DeliveryReconcileDraft| {
        if commands.contains(&draft.delivery_id) {
            return;
        }
        if reconcile_draft()
            .as_ref()
            .is_some_and(|current| current.delivery_id == draft.delivery_id)
        {
            reconcile_draft.set(Some(draft));
        }
    };
    let confirm_action = move |request: DeliveryActionRequest| {
        if commands.any_busy() || commands.contains(&request.delivery_id) {
            return;
        }
        if pending_dialog().is_some_and(|dialog| {
            is_mail_disposition_conflict(&dialog.delivery)
                && request.action != DeliveryAction::ReconcileFailed
        }) {
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
        let submitted = commands.submit(
            delivery_id.clone(),
            t!("delivery-confirm"),
            move |command_id| {
                let delivery_id = delivery_id.clone();
                let external_reference = external_reference.clone();
                let failure_code = failure_code.clone();
                async move {
                    execute_delivery(
                        command_id,
                        delivery_id,
                        action,
                        expected_fencing_version,
                        external_reference,
                        failure_code,
                    )
                    .await
                    .map_err(|error| error.code)
                }
            },
            move |_| {
                set_if_alive(notice, Some(AdminActionFeedback::Success));
                set_if_alive(success_action, Some(action));
            },
            move |code| {
                // 新的版本拒绝不能抹掉更早一次未知；原 fence 与 UUID 仍由宿主保留。
                if matches!(code.as_str(), "stale_delivery_state" | "stale_state_version") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        if submitted {
            pending_dialog.set(None);
            reconcile_draft.set(None);
        }
    };

    let snapshot = deliveries.read_unchecked();
    let (page, load_error) = match snapshot.as_ref() {
        None => (None, None),
        Some(Err(error)) => (None, Some(admin_api_error_message(error))),
        Some(Ok(page)) => (Some(page), None),
    };
    let pending = pending_dialog();
    let draft = reconcile_draft();
    let row_dialog = pending.clone().filter(|dialog| dialog.kind == DeliveryDialogKind::Reconcile);
    let page_dialog = pending.filter(|dialog| dialog.kind != DeliveryDialogKind::Reconcile);
    let success_message = delivery_success_message(success_action());
    let mut on_status_change = query.clone();
    let mut on_page_change = query.clone();
    let mut on_sort_change = query.clone();
    let mut on_filters_change = query;

    rsx! {
        section { class: "admin-delivery-page", aria_labelledby: "delivery-title",
            h1 { id: "delivery-title", {t!("page-deliveries-title")} }
            PendingCommandPanel { owner: commands }
            form { class: "admin-delivery-lookup", onsubmit: open_lookup,
                label { r#for: "delivery-lookup-id",
                    span { {t!("delivery-detail-lookup-label")} }
                    input {
                        id: "delivery-lookup-id",
                        r#type: "text",
                        value: lookup(),
                        oninput: move |event| {
                            lookup.set(event.value());
                            lookup_invalid.set(false);
                        },
                    }
                }
                button { r#type: "submit", {t!("delivery-detail-lookup-open")} }
                if lookup_invalid() {
                    p { class: "admin-delivery-error", role: "alert",
                        {t!("delivery-detail-lookup-invalid")}
                    }
                }
            }
            DeliveryFilters {
                status: query_state().status,
                recipient: query_state().recipient,
                reference: query_state().reference,
                template: query_state().template,
                category: query_state().category,
                world: query_state().world,
                created_from: query_state().created_from,
                created_to: query_state().created_to,
                on_status_change: move |status| {
                    // 选择状态后立即回到第一页并保留排序，由页面 push 到地址栏。
                    let mut next = query_state();
                    next.status = status;
                    next.page = 1;
                    on_status_change.push(next);
                },
                on_filters_change: move |filters: DeliveryFiltersDraft| {
                    // 提交筛选后回到第一页；空值由 query 序列化自动省略。
                    let mut next = query_state();
                    next.recipient = filters.recipient;
                    next.reference = filters.reference;
                    next.template = filters.template;
                    next.category = filters.category;
                    next.world = filters.world;
                    next.created_from = filters.created_from;
                    next.created_to = filters.created_to;
                    next.page = 1;
                    on_filters_change.push(next);
                },
            }
            if let Some(page) = page {
                DeliveryTable {
                    page: page.clone(),
                    sort: query_state().sort,
                    direction: query_state().direction,
                    on_sort_change: move |(sort, direction)| {
                        let mut next = query_state();
                        next.sort = sort;
                        next.direction = direction;
                        on_sort_change.push(next);
                    },
                    on_open: open_dialog,
                    pending_dialog: row_dialog,
                    draft: draft.clone(),
                    on_draft_change: update_reconcile_draft,
                    on_confirm: confirm_action,
                    on_cancel: move |_| {
                        pending_dialog.set(None);
                        reconcile_draft.set(None);
                    },
                }
            } else if let Some(error) = load_error {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: error,
                    detail: "",
                    retry_label: t!("admin-retry"),
                    on_retry: move |_| refresh.set(refresh().saturating_add(1)),
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
                AdminPager {
                    page: page.pagination.page,
                    page_size: page.pagination.page_size,
                    total: Some(page.pagination.total),
                    pagination,
                    label: t!("delivery-pagination"),
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.page = next_page;
                        on_page_change.push(next);
                    },
                }
            }
            if let Some(dialog) = page_dialog {
                DeliveryDialog {
                    delivery: dialog.delivery.clone(),
                    kind: dialog.kind,
                    busy: commands.any_busy(),
                    draft: draft.unwrap_or_else(|| {
                        DeliveryReconcileDraft::for_delivery(&dialog.delivery)
                    }),
                    on_draft_change: move |_| {},
                    on_confirm: confirm_action,
                    on_cancel: move |_| {
                        pending_dialog.set(None);
                        reconcile_draft.set(None);
                    },
                }
            }
        }
    }
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
