//! Admin 邮件轮询状态与入站审查页面。
//!
//! 页面只展示后端安全投影；筛选条件、分页和详情读取都通过冻结的 HTTP 契约完成，浏览器
//! 不解析邮件正文，也不自行推导邮件处理状态。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{
    AdminMailIntake, AdminMailIntakeDetail, AdminMailIntakePage, AdminMailIntakeQuery,
    AdminMailPolling, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
    set_mail_paused, trigger_mail_poll,
};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel, format_admin_time};

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub(super) fn page() -> Element {
    let mut mailbox_key = use_signal(String::new);
    let mut world_filter = use_signal(String::new);
    let mut kind_filter = use_signal(String::new);
    let mut status_filter = use_signal(String::new);
    let mut decision_filter = use_signal(String::new);
    let mut received_from = use_signal(String::new);
    let mut received_to = use_signal(String::new);
    let mut active_mailbox = use_signal(String::new);
    // 旧版邮件页首次打开显示所有邮箱的入站记录；轮询状态只有输入具体邮箱后才读取。
    let mut active_query = use_signal(AdminMailIntakeQuery::default);
    let mut page_number = use_signal(|| 1_i64);
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<bool>);
    let mut selected_intake = use_signal(|| None::<String>);

    let polling = use_resource(move || {
        let _ = refresh();
        let mailbox = active_mailbox();
        async move {
            if mailbox.is_empty() { Ok(None) } else { fetch_mail_state(mailbox).await.map(Some) }
        }
    });
    let intakes = use_resource(move || {
        let _ = refresh();
        fetch_mail_intakes(active_query(), page_number())
    });
    let detail = use_resource(move || {
        let _ = refresh();
        let intake_id = selected_intake();
        async move {
            match intake_id {
                Some(intake_id) => fetch_mail_intake_detail(intake_id).await.map(Some),
                None => Ok(None),
            }
        }
    });

    let load_filters = move |_| {
        let next_mailbox = mailbox_key().trim().to_owned();
        active_mailbox.set(next_mailbox.clone());
        active_query.set(AdminMailIntakeQuery {
            mailbox_key: next_mailbox,
            world_id: world_filter().trim().to_owned(),
            kind: kind_filter().trim().to_owned(),
            status: status_filter().trim().to_owned(),
            decision: decision_filter().trim().to_owned(),
            received_from: received_from().trim().to_owned(),
            received_to: received_to().trim().to_owned(),
        });
        page_number.set(1);
        selected_intake.set(None);
        notice.set(None);
        refresh.set(refresh().saturating_add(1));
    };

    match (&*polling.read_unchecked(), &*intakes.read_unchecked()) {
        (None, _) | (_, None) => rsx! {
            AdminPanel { title: t!("page-mails-title"), detail: t!("admin-mail-loading") }
        },
        (Some(Err(_)), _) | (_, Some(Err(_))) => rsx! {
            AdminLoadError {
                title: t!("page-mails-title"),
                detail: t!("admin-mail-error"),
                refresh,
            }
        },
        (Some(Ok(polling)), Some(Ok(intakes))) => {
            let (detail_value, detail_loading, detail_error) = match &*detail.read_unchecked() {
                None => (None, true, false),
                Some(Err(_)) => (None, false, true),
                Some(Ok(value)) => (value.clone(), false, false),
            };

            rsx! {
                section { class: "admin-panel admin-mail-page", aria_labelledby: "admin-mail-title",
                    h1 { id: "admin-mail-title", {t!("page-mails-title")} }
                    div { class: "admin-panel__actions admin-mail-controls",
                        label {
                            {t!("admin-mail-mailbox")}
                            input {
                                maxlength: "256",
                                value: mailbox_key(),
                                oninput: move |event| mailbox_key.set(event.value()),
                            }
                        }
                        button { r#type: "button", onclick: load_filters, {t!("admin-mail-load")} }
                        MailTriggerButton { notice, refresh }
                    }
                    section { class: "admin-mail-filters", aria_labelledby: "admin-mail-filters-title",
                        h2 { id: "admin-mail-filters-title", {t!("admin-mail-filters-title")} }
                        div { class: "admin-panel__actions admin-mail-filter-controls",
                            label {
                                class: "admin-mail-world-filter",
                                {t!("admin-mail-intake-world")}
                                select {
                                value: world_filter(),
                                onchange: move |event| {
                                    let world_id = event.value();
                                    world_filter.set(world_id.clone());
                                    activate_mail_query(
                                        active_query,
                                        page_number,
                                        selected_intake,
                                        notice,
                                        refresh,
                                        AdminMailIntakeQuery {
                                            mailbox_key: mailbox_key().trim().to_owned(),
                                            world_id,
                                            kind: kind_filter().trim().to_owned(),
                                            status: status_filter().trim().to_owned(),
                                            decision: decision_filter().trim().to_owned(),
                                            received_from: received_from().trim().to_owned(),
                                            received_to: received_to().trim().to_owned(),
                                        },
                                    );
                                },
                                    option { value: "", {t!("admin-mail-all")} }
                                    option { value: "main", {t!("admin-auctions-world-main")} }
                                    option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                                }
                            }
                            label {
                                class: "admin-mail-kind-filter",
                                {t!("admin-mail-intake-kind")}
                                select {
                                value: kind_filter(),
                                onchange: move |event| {
                                    let kind = event.value();
                                    kind_filter.set(kind.clone());
                                    activate_mail_query(
                                        active_query,
                                        page_number,
                                        selected_intake,
                                        notice,
                                        refresh,
                                        AdminMailIntakeQuery {
                                            mailbox_key: mailbox_key().trim().to_owned(),
                                            world_id: world_filter().trim().to_owned(),
                                            kind,
                                            status: status_filter().trim().to_owned(),
                                            decision: decision_filter().trim().to_owned(),
                                            received_from: received_from().trim().to_owned(),
                                            received_to: received_to().trim().to_owned(),
                                        },
                                    );
                                },
                                    option { value: "", {t!("admin-mail-all")} }
                                    option { value: "equipment", {t!("admin-mail-kind-equipment")} }
                                    option { value: "bid", {t!("admin-mail-kind-bid")} }
                                    option { value: "delivery", {t!("admin-mail-kind-delivery")} }
                                    option { value: "registration", {t!("admin-mail-kind-registration")} }
                                    option { value: "rebind", {t!("admin-mail-kind-rebind")} }
                                    option { value: "unknown", {t!("admin-mail-kind-unknown")} }
                                }
                            }
                            label {
                                class: "admin-mail-status-filter",
                                {t!("admin-mail-intake-status")}
                                select {
                                value: status_filter(),
                                onchange: move |event| {
                                    let status = event.value();
                                    status_filter.set(status.clone());
                                    activate_mail_query(
                                        active_query,
                                        page_number,
                                        selected_intake,
                                        notice,
                                        refresh,
                                        AdminMailIntakeQuery {
                                            mailbox_key: mailbox_key().trim().to_owned(),
                                            world_id: world_filter().trim().to_owned(),
                                            kind: kind_filter().trim().to_owned(),
                                            status,
                                            decision: decision_filter().trim().to_owned(),
                                            received_from: received_from().trim().to_owned(),
                                            received_to: received_to().trim().to_owned(),
                                        },
                                    );
                                },
                                    option { value: "", {t!("admin-mail-status-all")} }
                                    option { value: "accepted", {t!("admin-mail-status-accepted")} }
                                    option { value: "rejected", {t!("admin-mail-status-rejected")} }
                                    option { value: "failed", {t!("admin-mail-status-failed")} }
                                }
                            }
                            label {
                                {t!("admin-mail-filter-decision")}
                                input {
                                    maxlength: "128",
                                    value: decision_filter(),
                                    oninput: move |event| decision_filter.set(event.value()),
                                }
                            }
                            label {
                                {t!("admin-mail-filter-from")}
                                input {
                                    r#type: "datetime-local",
                                    value: received_from(),
                                    oninput: move |event| received_from.set(event.value()),
                                }
                            }
                            label {
                                {t!("admin-mail-filter-to")}
                                input {
                                    r#type: "datetime-local",
                                    value: received_to(),
                                    oninput: move |event| received_to.set(event.value()),
                                }
                            }
                            button { r#type: "button", onclick: load_filters, {t!("admin-mail-apply-filter")} }
                        }
                    }
                    if let Some(polling) = polling.clone() {
                        MailPollingCard { polling, notice, refresh }
                    } else {
                        p { class: "admin-panel__note", {t!("admin-mail-polling-not-loaded")} }
                    }
                    MailIntakeTable {
                        page: intakes.clone(),
                        page_number,
                        on_select: move |intake_id| selected_intake.set(Some(intake_id)),
                    }
                    MailIntakeDetailPanel {
                        visible: selected_intake().is_some(),
                        detail: detail_value,
                        loading: detail_loading,
                        error: detail_error,
                        refresh,
                        on_close: move |_| selected_intake.set(None),
                    }
                    if let Some(successful) = notice() {
                        p { class: "admin-panel__note action-feedback", role: "status",
                            if successful { {t!("admin-mail-action-success")} }
                            else { {t!("admin-mail-action-error")} }
                        }
                    }
                }
            }
        }
    }
}

/// 让旧版的闭集筛选在选择变化后立即生效，同时清理详情和操作反馈。
fn activate_mail_query(
    mut active_query: Signal<AdminMailIntakeQuery>,
    mut page_number: Signal<i64>,
    mut selected_intake: Signal<Option<String>>,
    mut notice: Signal<Option<bool>>,
    mut refresh: Signal<u64>,
    query: AdminMailIntakeQuery,
) {
    active_query.set(query);
    page_number.set(1);
    selected_intake.set(None);
    notice.set(None);
    refresh.set(refresh().saturating_add(1));
}

#[component]
fn MailTriggerButton(mut notice: Signal<Option<bool>>, mut refresh: Signal<u64>) -> Element {
    let mut busy = use_signal(|| false);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    rsx! {
        button {
            class: "admin-btn-primary",
            r#type: "button",
            disabled: busy(),
            onclick: move |_| {
                if busy() {
                    return;
                }
                busy.set(true);
                notice.set(None);
                let command_id = stable_command_id(command_state, "trigger".to_owned());
                let mut busy = busy;
                let mut notice = notice;
                let mut refresh = refresh;
                spawn(async move {
                    match trigger_mail_poll(command_id).await {
                        Ok(_) => {
                            notice.set(Some(true));
                            command_state.set(None);
                            refresh.set(refresh().saturating_add(1));
                        }
                        Err(_) => notice.set(Some(false)),
                    }
                    busy.set(false);
                });
            },
            if busy() { {t!("admin-action-working")} } else { {t!("admin-mail-trigger")} }
        }
    }
}

#[component]
fn MailPollingCard(
    polling: AdminMailPolling,
    mut notice: Signal<Option<bool>>,
    mut refresh: Signal<u64>,
) -> Element {
    let desired_paused = !polling.paused;
    let label = if polling.paused { t!("admin-mail-resume") } else { t!("admin-mail-pause") };
    let state_label =
        if polling.paused { t!("admin-mail-paused") } else { t!("admin-mail-running") };
    let mut busy = use_signal(|| false);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let pause_polling = polling.clone();
    let pause_notice = notice;
    let pause_refresh = refresh;
    rsx! {
        section {
            class: "admin-panel admin-mail-polling-card",
            aria_labelledby: "admin-mail-polling-title",
            aria_busy: busy().to_string(),
            h2 { id: "admin-mail-polling-title", {t!("admin-mail-polling-title")} }
            p { class: "admin-panel__note",
                {t!(
                    "admin-mail-polling-summary",
                    mailbox: polling.mailbox_key.clone(),
                    status: state_label,
                    version: polling.state_version,
                    failures: polling.consecutive_failures
                )}
            }
            p { class: "admin-panel__note",
                {t!(
                    "admin-mail-polling-times",
                    last: format_admin_time(&polling.last_poll_at),
                    next: format_admin_time(&polling.next_poll_at)
                )}
            }
            button {
                r#type: "button",
                disabled: busy(),
                onclick: move |_| {
                    if busy() {
                        return;
                    }
                    busy.set(true);
                    let polling = pause_polling.clone();
                    let command_id = stable_command_id(
                        command_state,
                        format!("pause:{}:{}:{}", polling.mailbox_key, polling.state_version, desired_paused),
                    );
                    let mut notice = pause_notice;
                    let mut refresh = pause_refresh;
                    let mut busy = busy;
                    spawn(async move {
                        match set_mail_paused(command_id, polling, desired_paused).await {
                            Ok(_) => {
                                notice.set(Some(true));
                                command_state.set(None);
                                refresh.set(refresh().saturating_add(1));
                            }
                            Err(_) => notice.set(Some(false)),
                        }
                        busy.set(false);
                    });
                },
                if busy() { {t!("admin-action-working")} } else { "{label}" }
            }
        }
    }
}

#[component]
fn MailIntakeTable(
    page: AdminMailIntakePage,
    mut page_number: Signal<i64>,
    on_select: EventHandler<String>,
) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-mail-intake-title",
            h2 { id: "admin-mail-intake-title", {t!("admin-mail-intake-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-intake-total", total: page.pagination.total)} }
            if page.intakes.is_empty() {
                p { class: "admin-panel__note", {t!("admin-mail-empty")} }
            } else {
                div { class: "public-auction-catalog admin-mail__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-mail-external-id")} }
                                th { {t!("admin-mail-intake-world")} }
                                th { {t!("admin-mail-intake-kind")} }
                                th { {t!("admin-mail-intake-status")} }
                                th { {t!("admin-mail-intake-decision")} }
                                th { {t!("admin-mail-intake-received")} }
                                th { {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for intake in page.intakes {
                                MailIntakeRow { intake, on_select }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.pagination.page,
                page_size: page.pagination.page_size,
                total: page.pagination.total,
                label: t!("admin-mail-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}

#[component]
fn MailIntakeRow(intake: AdminMailIntake, on_select: EventHandler<String>) -> Element {
    let intake_id = intake.id.clone();
    rsx! {
        tr { class: "admin-mail__row",
            td { "data-label": t!("admin-mail-external-id"), TruncatedText { value: intake.external_mail_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-world"), TruncatedText { value: intake.world_id, size: "compact" } }
            td { "data-label": t!("admin-mail-intake-kind"), TruncatedText { value: mail_kind_label(&intake.kind) } }
            td { "data-label": t!("admin-mail-intake-status"),
                span { class: mail_status_class(&intake.status), TruncatedText { value: mail_status_label(&intake.status), size: "compact" } }
            }
            td { "data-label": t!("admin-mail-intake-decision"),
                span { class: mail_decision_class(&intake.decision_code), "{intake.decision_code}" }
            }
            td {
                "data-label": t!("admin-mail-intake-received"),
                time { datetime: intake.received_at.clone(), "{format_admin_time(&intake.received_at)}" }
            }
            td { "data-label": t!("catalog-actions"),
                button {
                    r#type: "button",
                    onclick: move |_| on_select.call(intake_id.clone()),
                    {t!("admin-mail-open-detail")}
                }
            }
        }
    }
}

fn mail_status_class(status: &str) -> &'static str {
    match status {
        "accepted" => "admin-mail__status admin-mail__status--accepted",
        "failed" => "admin-mail__status admin-mail__status--failed",
        _ => "admin-mail__status admin-mail__status--pending",
    }
}

fn mail_kind_label(kind: &str) -> String {
    match kind {
        "equipment" => t!("admin-mail-kind-equipment"),
        "bid" => t!("admin-mail-kind-bid"),
        "delivery" => t!("admin-mail-kind-delivery"),
        "registration" => t!("admin-mail-kind-registration"),
        "rebind" => t!("admin-mail-kind-rebind"),
        "unknown" => t!("admin-mail-kind-unknown"),
        _ => kind.to_owned(),
    }
}

fn mail_status_label(status: &str) -> String {
    match status {
        "accepted" => t!("admin-mail-status-accepted"),
        "rejected" => t!("admin-mail-status-rejected"),
        "failed" => t!("admin-mail-status-failed"),
        _ => status.to_owned(),
    }
}

fn mail_decision_class(decision: &str) -> &'static str {
    if decision.contains("reject") || decision.contains("fail") {
        "admin-mail__decision admin-mail__decision--rejected"
    } else if decision.contains("accept") || decision.contains("success") {
        "admin-mail__decision admin-mail__decision--accepted"
    } else {
        "admin-mail__decision admin-mail__decision--pending"
    }
}

#[component]
fn MailIntakeDetailPanel(
    visible: bool,
    detail: Option<AdminMailIntakeDetail>,
    loading: bool,
    error: bool,
    mut refresh: Signal<u64>,
    on_close: EventHandler<()>,
) -> Element {
    if !visible {
        return rsx! {};
    }

    if loading {
        return rsx! {
            section { class: "admin-panel admin-mail-detail", aria_live: "polite",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-detail-loading")} }
            }
        };
    }

    if error {
        return rsx! {
            section { class: "admin-panel admin-mail-detail", role: "alert",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-detail-error")} }
                button {
                    r#type: "button",
                    onclick: move |_| refresh.set(refresh().saturating_add(1)),
                    {t!("admin-retry")}
                }
                button { r#type: "button", onclick: move |_| on_close.call(()), {t!("admin-mail-close-detail")} }
            }
        };
    }

    let Some(detail) = detail else {
        return rsx! {
            section { class: "admin-panel admin-mail-detail",
                h2 { {t!("admin-mail-detail-title")} }
                p { class: "admin-panel__note", {t!("admin-mail-no-selection")} }
            }
        };
    };

    let metadata = detail.metadata;
    let warnings = if metadata.warning_codes.is_empty() {
        t!("value-empty")
    } else {
        metadata.warning_codes.join(", ")
    };
    let delivery_ids = if metadata.delivery_ids.is_empty() {
        t!("value-empty")
    } else {
        metadata.delivery_ids.join(", ")
    };

    rsx! {
        section { class: "admin-panel admin-mail-detail", aria_labelledby: "admin-mail-detail-title",
            header { class: "admin-mail-detail__header",
                h2 { id: "admin-mail-detail-title", {t!("admin-mail-detail-title")} }
                button { r#type: "button", onclick: move |_| on_close.call(()), {t!("admin-mail-close-detail")} }
            }
            dl { class: "admin-mail-detail__grid",
                dt { {t!("admin-mail-intake-id")} }
                dd { "{detail.id}" }
                dt { {t!("admin-mail-external-id")} }
                dd { "{detail.external_mail_id}" }
                dt { {t!("admin-mail-mailbox")} }
                dd { "{detail.mailbox_key}" }
                dt { {t!("admin-mail-intake-world")} }
                dd { "{detail.world_id}" }
                dt { {t!("admin-mail-intake-kind")} }
                dd { "{mail_kind_label(&detail.kind)}" }
                dt { {t!("admin-mail-intake-status")} }
                dd {
                    span { class: mail_status_class(&detail.status),
                        {mail_status_label(&detail.status)}
                    }
                }
                dt { {t!("admin-mail-intake-decision")} }
                dd { "{detail.decision_code}" }
                dt { {t!("admin-mail-intake-received")} }
                dd { "{format_admin_time(&detail.received_at)}" }
                dt { {t!("admin-mail-processed")} }
                dd { "{format_admin_time(&detail.processed_at)}" }
                dt { {t!("admin-mail-auction-id")} }
                dd { "{metadata.auction_id}" }
                dt { {t!("admin-mail-submission-id")} }
                dd { "{metadata.submission_id}" }
                dt { {t!("admin-mail-user-id")} }
                dd { "{metadata.user_id}" }
                dt { {t!("admin-mail-added-count")} }
                dd { "{metadata.added_count}" }
                dt { {t!("admin-mail-existing-count")} }
                dd { "{metadata.existing_count}" }
                dt { {t!("admin-mail-preview-revision")} }
                dd { "{metadata.preview_revision}" }
                dt { {t!("admin-mail-attachment-count")} }
                dd { "{metadata.attachment_count}" }
                dt { {t!("admin-mail-owner-result")} }
                dd { "{metadata.owner_result_code}" }
                dt { {t!("admin-mail-reason-code")} }
                dd { "{metadata.reason_code}" }
                dt { {t!("admin-mail-warnings")} }
                dd { "{warnings}" }
                dt { {t!("admin-mail-delivery-ids")} }
                dd { "{delivery_ids}" }
            }
        }
    }
}

fn stable_command_id(mut command_state: Signal<Option<(String, Uuid)>>, signature: String) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}
