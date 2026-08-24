//! Admin delivery outbox 页面。
//!
//! 页面只操作后端返回的安全投影，并把快照中的 fencing version 原样回传。外部传输结果不由
//! 浏览器推断；未知结果只能显示为人工对账状态，具体证明由服务端命令端口校验。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::{TruncatedText, format_credit_amount as format_amount};

use crate::api::{Delivery, DeliveryAction, DeliveryPage, execute_delivery, fetch_deliveries};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel};

const DELIVERY_STATUS_VALUES: [&str; 7] =
    ["prepared", "pending", "claimed", "sending", "succeeded", "failed", "manual_reconcile"];

pub(super) fn page() -> Element {
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

#[component]
fn DeliveryFilters(mut status_filter: Signal<String>, mut page_number: Signal<i64>) -> Element {
    rsx! {
        section { class: "admin-delivery__filters", aria_labelledby: "delivery-filter-label",
            label { id: "delivery-filter-label",
                {t!("delivery-filter")}
                select {
                    value: status_filter(),
                    onchange: move |event| {
                        status_filter.set(event.value());
                        page_number.set(1);
                    },
                    option { value: "", {t!("delivery-all")} }
                    for status in DELIVERY_STATUS_VALUES {
                        option {
                            value: status,
                            {delivery_status_label(status)}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeliveryTable(
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

#[component]
fn DeliveryRow(
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

#[component]
fn DeliveryPager(page: DeliveryPage, mut page_number: Signal<i64>) -> Element {
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

#[derive(Debug, Clone, PartialEq)]
struct DeliveryDialogState {
    delivery: Delivery,
    kind: DeliveryDialogKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryDialogKind {
    Release,
    Retry,
    Reconcile,
}

impl DeliveryDialogKind {
    const fn action(self) -> DeliveryAction {
        match self {
            Self::Release => DeliveryAction::Release,
            Self::Retry => DeliveryAction::Retry,
            Self::Reconcile => DeliveryAction::ReconcileRetry,
        }
    }

    fn action_label(self) -> String {
        match self {
            Self::Release => t!("delivery-release"),
            Self::Retry => t!("delivery-retry"),
            Self::Reconcile => t!("delivery-reconcile"),
        }
    }

    fn title(self) -> String {
        match self {
            Self::Release => t!("delivery-confirm-release-title"),
            Self::Retry => t!("delivery-confirm-retry-title"),
            Self::Reconcile => t!("delivery-reconcile-title"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeliveryActionRequest {
    delivery_id: String,
    action: DeliveryAction,
    expected_fencing_version: i64,
    external_reference: Option<String>,
    failure_code: Option<String>,
}

#[component]
fn DeliveryDialog(
    delivery: Delivery,
    kind: DeliveryDialogKind,
    busy: bool,
    on_confirm: EventHandler<DeliveryActionRequest>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut reconcile_action = use_signal(|| "retry".to_owned());
    let mut external_reference = use_signal(String::new);
    let mut failure_code = use_signal(String::new);
    let title = kind.title();
    let delivery_id = delivery.id.clone();
    let expected_fencing_version = delivery.fencing_version;
    let status = delivery_status_label(&delivery.status);
    let failure = delivery.failure_code.clone().unwrap_or_else(|| t!("value-empty"));

    rsx! {
        div { class: "admin-delivery__dialog-backdrop", role: "presentation",
            section {
                class: "admin-delivery__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "delivery-dialog-title",
                aria_busy: busy.to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy {
                        on_cancel.call(());
                    }
                },
                h2 { id: "delivery-dialog-title", {title} }
                p { class: "admin-panel__note", "{delivery.recipient} · {status} · {failure}" }
                if kind == DeliveryDialogKind::Reconcile {
                    form {
                        onsubmit: move |event| {
                            event.prevent_default();
                            if busy {
                                return;
                            }
                            let selected_action = reconcile_action();
                            let external = external_reference().trim().to_owned();
                            let failure = failure_code().trim().to_owned();
                            if selected_action == "succeeded" && external.is_empty()
                                || selected_action == "failed" && failure.is_empty()
                            {
                                return;
                            }
                            let action = match selected_action.as_str() {
                                "succeeded" => DeliveryAction::ReconcileSucceeded,
                                "failed" => DeliveryAction::ReconcileFailed,
                                _ => DeliveryAction::ReconcileRetry,
                            };
                            on_confirm.call(DeliveryActionRequest {
                                delivery_id: delivery_id.clone(),
                                action,
                                expected_fencing_version,
                                external_reference: (action == DeliveryAction::ReconcileSucceeded)
                                    .then_some(external),
                                failure_code: (action == DeliveryAction::ReconcileFailed)
                                    .then_some(failure),
                            });
                        },
                        label {
                            {t!("delivery-reconcile-action")}
                            select {
                                value: reconcile_action(),
                                disabled: busy,
                                onchange: move |event| reconcile_action.set(event.value()),
                                option { value: "retry", {t!("delivery-reconcile-action-retry")} }
                                option { value: "succeeded", {t!("delivery-reconcile-action-succeeded")} }
                                option { value: "failed", {t!("delivery-reconcile-action-failed")} }
                            }
                        }
                        if reconcile_action() == "succeeded" {
                            label {
                                {t!("delivery-external-reference")}
                                input {
                                    r#type: "text",
                                    maxlength: "256",
                                    required: true,
                                    disabled: busy,
                                    value: external_reference(),
                                    oninput: move |event| external_reference.set(event.value()),
                                }
                            }
                        }
                        if reconcile_action() == "failed" {
                            label {
                                {t!("delivery-failure-code")}
                                input {
                                    r#type: "text",
                                    maxlength: "128",
                                    required: true,
                                    disabled: busy,
                                    value: failure_code(),
                                    oninput: move |event| failure_code.set(event.value()),
                                }
                            }
                        }
                        DeliveryDialogActions {
                            busy,
                            confirm_label: t!("delivery-reconcile"),
                            on_cancel,
                        }
                    }
                } else {
                    div { class: "admin-delivery__dialog-actions",
                        button {
                            r#type: "button",
                            class: "admin-btn-secondary",
                            disabled: busy,
                            autofocus: true,
                            onclick: move |_| on_cancel.call(()),
                            {t!("delivery-cancel")}
                        }
                        button {
                            r#type: "button",
                            class: "admin-btn-primary",
                            disabled: busy,
                            onclick: move |_| on_confirm.call(DeliveryActionRequest {
                                delivery_id: delivery_id.clone(),
                                action: kind.action(),
                                expected_fencing_version,
                                external_reference: None,
                                failure_code: None,
                            }),
                            if busy { {t!("admin-action-working")} } else { {t!("delivery-confirm")} }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DeliveryDialogActions(
    busy: bool,
    confirm_label: String,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-delivery__dialog-actions",
            button {
                r#type: "button",
                class: "admin-btn-secondary",
                disabled: busy,
                autofocus: true,
                onclick: move |_| on_cancel.call(()),
                {t!("delivery-cancel")}
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy,
                if busy { {t!("admin-action-working")} } else { {confirm_label} }
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

fn delivery_origin_label(origin_kind: &str) -> String {
    match origin_kind {
        "settlement" => t!("delivery-origin-settlement"),
        "settlement_recovery_action" => t!("delivery-origin-recovery"),
        "user_key" => t!("delivery-origin-user-key"),
        "manual" => t!("delivery-origin-manual"),
        "mail_intake" => t!("delivery-origin-mail-intake"),
        "operations_alert" => t!("delivery-origin-operations-alert"),
        _ => origin_kind.to_owned(),
    }
}

fn delivery_status_class(status: &str) -> &'static str {
    match status {
        "succeeded" => "admin-delivery__status admin-delivery__status--succeeded",
        "failed" => "admin-delivery__status admin-delivery__status--failed",
        "manual_reconcile" => "admin-delivery__status admin-delivery__status--manual",
        "sending" | "claimed" => "admin-delivery__status admin-delivery__status--active",
        "prepared" | "pending" => "admin-delivery__status admin-delivery__status--pending",
        _ => "admin-delivery__status",
    }
}
