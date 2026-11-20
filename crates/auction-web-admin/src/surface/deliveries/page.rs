//! Delivery 页面状态、查询资源和动作编排。

use std::future::Future;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind};
use dioxus::prelude::*;
use uuid::Uuid;

use super::super::query::use_deliveries_query_state;
use super::delivery_command_entity;
use super::dialog::{DIALOG_ID, DeliveryDialog};
use super::filters::{DeliveryFilters, DeliveryFiltersDraft};
use super::labels::action_feedback_message;
use super::modal::close_delivery_modal;
use super::model::{
    DeliveryActionRequest, DeliveryDialogKind, DeliveryDialogState, DeliveryReconcileDraft,
};
use super::table::DeliveryTable;
use crate::api::{
    AdminWriteFailure, Delivery, DeliveryAction, execute_delivery, fetch_deliveries, valid_uuid,
};
use crate::surface::pagination::{AdminPager, use_admin_page, use_admin_pagination};
use crate::surface::pending::{
    PendingCommandPanel, PendingCommands, set_if_alive, use_pending_commands,
};
use crate::surface::{AdminActionFeedback, admin_api_error_message};

/// 加载 delivery 安全投影并编排带 fencing 的操作员动作。
pub fn page() -> Element {
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut success_action = use_signal(|| None::<DeliveryAction>);
    let commands = use_pending_commands(refresh, |entity| entity.starts_with("delivery:"));
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
        if !commands.any_busy() && !commands.contains(&delivery_command_entity(&delivery.id)) {
            notice.set(None);
            success_action.set(None);
            if kind == DeliveryDialogKind::Reconcile
                && reconcile_draft().as_ref().is_none_or(|draft| draft.delivery_id != delivery.id)
            {
                reconcile_draft.set(Some(DeliveryReconcileDraft::for_delivery(&delivery)));
            }
            pending_dialog.set(Some(DeliveryDialogState { delivery, kind }));
        }
    };
    let update_reconcile_draft = move |draft: DeliveryReconcileDraft| {
        if commands.contains(&delivery_command_entity(&draft.delivery_id)) {
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
        if commands.any_busy() || commands.contains(&delivery_command_entity(&request.delivery_id))
        {
            return;
        }
        let Some(dialog) = pending_dialog() else { return };
        if dialog.delivery.id != request.delivery_id
            || dialog.delivery.fencing_version != request.expected_fencing_version
        {
            return;
        }
        if request.action == DeliveryAction::ReconcileRetry
            && dialog.kind == DeliveryDialogKind::Reconcile
        {
            // 旧版 retry 人工对账先关闭字段框，再进入共用的第二确认框；此时不能发 mutation。
            pending_dialog.set(Some(DeliveryDialogState {
                delivery: dialog.delivery,
                kind: DeliveryDialogKind::ReconcileRetry,
            }));
            return;
        }
        notice.set(None);
        success_action.set(None);
        let submitted = submit_queue_action(
            commands,
            request,
            refresh,
            notice,
            success_action,
            |id, request| {
                execute_delivery(
                    id,
                    request.delivery_id,
                    request.action,
                    request.expected_fencing_version,
                    request.external_reference,
                    request.failure_code,
                )
            },
        );
        if submitted {
            close_delivery_modal(DIALOG_ID);
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
    // 行内对账草稿由对话框所在行订阅；页面只在页级确认框打开时读取，表格只转交信号，
    // 因此输入外部引用时页面、表格与其他行都不重渲染。
    let row_dialog = pending.clone().filter(|dialog| {
        matches!(dialog.kind, DeliveryDialogKind::Reconcile | DeliveryDialogKind::ReconcileRetry)
    });
    let page_dialog = pending.filter(|dialog| {
        !matches!(dialog.kind, DeliveryDialogKind::Reconcile | DeliveryDialogKind::ReconcileRetry)
    });
    let success_message = delivery_success_message(success_action());
    // 渲染期只取一次地址栏状态快照；事件回调仍各自读取最新值。
    let current = query_state();
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
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        {t!("delivery-detail-lookup-invalid")}
                    }
                }
            }
            DeliveryFilters {
                status: current.status,
                recipient: current.recipient,
                reference: current.reference,
                template: current.template,
                category: current.category,
                world: current.world,
                created_from: current.created_from,
                created_to: current.created_to,
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
                    sort: current.sort,
                    direction: current.direction,
                    on_sort_change: move |(sort, direction)| {
                        let mut next = query_state();
                        next.sort = sort;
                        next.direction = direction;
                        on_sort_change.push(next);
                    },
                    on_open: open_dialog,
                    pending_dialog: row_dialog,
                    draft: reconcile_draft,
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
                        "action-feedback action-feedback--success"
                    } else {
                        "action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {action_feedback_message(&feedback, success_message)}
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
                    draft: reconcile_draft().unwrap_or_else(|| {
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

fn submit_queue_action<F>(
    commands: PendingCommands,
    request: DeliveryActionRequest,
    refresh: Signal<u64>,
    notice: Signal<Option<AdminActionFeedback>>,
    success_action: Signal<Option<DeliveryAction>>,
    send: impl Fn(Uuid, DeliveryActionRequest) -> F + 'static,
) -> bool
where
    F: Future<Output = Result<Delivery, AdminWriteFailure>> + 'static,
{
    let action = request.action;
    commands.submit(
        delivery_command_entity(&request.delivery_id),
        match action {
            DeliveryAction::Release => t!("delivery-release"),
            DeliveryAction::Retry => t!("delivery-retry"),
            DeliveryAction::ReconcileRetry => t!("delivery-reconcile-action-retry"),
            DeliveryAction::ReconcileSucceeded => t!("delivery-reconcile-action-succeeded"),
            DeliveryAction::ReconcileFailed => t!("delivery-reconcile-action-failed"),
        },
        move |command_id| send(command_id, request.clone()),
        move |_| {
            set_if_alive(notice, Some(AdminActionFeedback::Success));
            set_if_alive(success_action, Some(action));
        },
        move |code| {
            // 新的版本拒绝不能抹掉更早一次未知；原 fence 与 UUID 仍由宿主保留。
            if matches!(
                code.as_str(),
                "stale_delivery_state" | "stale_state_version" | "delivery_recipient_renamed"
            ) {
                let mut refresh = refresh;
                refresh.set(refresh().saturating_add(1));
            }
            set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
        },
    )
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::*;
    use gloo_net::http::Method;

    use super::submit_queue_action;
    use crate::api::{AdminApiFailure, AdminWriteFailure, DeliveryAction, execute_delivery};
    use crate::surface::auctions::draft_page_tests::{
        button, click_button, command_button, has_live_text, init_i18n, mount_writer, settle_writer,
    };
    use crate::surface::deliveries::delivery_command_entity;
    use crate::surface::deliveries::model::DeliveryActionRequest;
    use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

    #[test]
    fn queue_rereads_after_stale_or_renamed_but_keeps_the_frozen_retry() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        const PATH: &str =
            "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/reconcile";
        let request = DeliveryActionRequest {
            delivery_id: ID.to_owned(),
            action: DeliveryAction::ReconcileFailed,
            expected_fencing_version: 4,
            external_reference: None,
            failure_code: Some("verified_not_sent".to_owned()),
        };
        for (code, body) in [
            (
                "stale_delivery_state",
                r#"{"code":"stale_delivery_state","message":"队列已变化","data":null,"request_id":null}"#,
            ),
            (
                "stale_state_version",
                r#"{"code":"stale_state_version","message":"队列已变化","data":null,"request_id":null}"#,
            ),
            (
                "delivery_recipient_renamed",
                r#"{"code":"delivery_recipient_renamed","message":"队列已变化","data":null,"request_id":null}"#,
            ),
        ] {
            let attempts = Rc::new(RefCell::new(Vec::new()));
            let (mut dom, _) = mount_writer({
                let attempts = attempts.clone();
                let request = request.clone();
                move || {
                    init_i18n();
                    let refresh = use_signal(|| 0_u64);
                    let notice = use_signal(|| None);
                    let success = use_signal(|| None);
                    let commands =
                        use_pending_commands(refresh, |entity| entity.starts_with("delivery:"));
                    let attempts = attempts.clone();
                    let request = request.clone();
                    rsx! {
                        p { "队列读取世代 {refresh()}" }
                        button {
                            onclick: move |_| {
                                let attempts = attempts.clone();
                                assert!(submit_queue_action(
                                    commands, request.clone(), refresh, notice, success,
                                    move |command_id, request| {
                                        let first = attempts.borrow().is_empty();
                                        attempts.borrow_mut().push((command_id, request.clone()));
                                        async move {
                                            if first {
                                                return Err(AdminWriteFailure::outcome_unknown("network_error"));
                                            }
                                            AdminApiFailure::stub_write_response(PATH, Method::POST, 409, body);
                                            execute_delivery(
                                                command_id, request.delivery_id, request.action,
                                                request.expected_fencing_version, request.external_reference,
                                                request.failure_code,
                                            ).await
                                        }
                                    },
                                ));
                            },
                            {"确认队列处置".to_owned()}
                        }
                        PendingCommandPanel { owner: commands }
                    }
                }
            });
            click_button(&mut dom, "确认队列处置");
            settle_writer(&mut dom);
            assert!(has_live_text(&dom, "队列读取世代 0"), "{code}");
            let (id, first_request) = attempts.borrow()[0].clone();
            assert_eq!(first_request, request);
            let (retry, disabled) = command_button(&dom, id, "原样重试").unwrap();
            assert!(!disabled, "{code}");

            dom.dispatch("click", retry, "");
            settle_writer(&mut dom);
            assert!(has_live_text(&dom, "队列读取世代 1"), "{code}");
            assert_eq!(
                attempts.borrow().as_slice(),
                [(id, request.clone()), (id, request.clone())]
            );
            assert!(!command_button(&dom, id, "原样重试").unwrap().1, "{code}");
        }
    }

    #[test]
    fn queue_http_conflict_keeps_the_earlier_unknown_command_id() {
        const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
        const PATH: &str = "/api/v1/admin/deliveries/0f8fad5b-d9cb-469f-a165-70867728950e/retry";
        for body in [
            r#"{"code":"stale_delivery_state","message":"队列已变化","data":null,"request_id":null}"#,
            r#"{"code":"delivery_recipient_renamed","message":"队列已变化","data":null,"request_id":null}"#,
        ] {
            let command_ids = Rc::new(RefCell::new(Vec::new()));
            let (mut dom, _) = mount_writer({
                let command_ids = command_ids.clone();
                move || {
                    init_i18n();
                    let command_ids = command_ids.clone();
                    let commands = use_pending_commands(use_signal(|| 0), |_| false);
                    rsx! {
                        button {
                            onclick: move |_| {
                                let command_ids = command_ids.clone();
                                assert!(commands.submit(
                                    delivery_command_entity(ID),
                                    "重试投递".to_owned(),
                                    move |command_id| {
                                        let command_ids = command_ids.clone();
                                        async move {
                                            let first = command_ids.borrow().is_empty();
                                            command_ids.borrow_mut().push(command_id);
                                            if first {
                                                AdminApiFailure::stub_write_response(PATH, Method::POST, 503, "{");
                                            } else {
                                                AdminApiFailure::stub_write_response(PATH, Method::POST, 409, body);
                                            }
                                            execute_delivery(command_id, ID.to_owned(), DeliveryAction::Retry, 4, None, None).await
                                        }
                                    },
                                    |_| panic!("非成功回执不能交付"),
                                    |_| {},
                                ));
                            },
                            {"重试投递".to_owned()}
                        }
                        PendingCommandPanel { owner: commands }
                    }
                }
            });
            click_button(&mut dom, "重试投递");
            settle_writer(&mut dom);
            assert!(!button(&dom, "原样重试").unwrap().1);
            click_button(&mut dom, "原样重试");
            settle_writer(&mut dom);
            let ids = command_ids.borrow();
            assert_eq!(ids.len(), 2);
            assert_eq!(ids[0], ids[1]);
            assert!(!button(&dom, "原样重试").unwrap().1);
        }
    }
}
