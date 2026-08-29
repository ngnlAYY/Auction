//! Admin 邮件页面加载、筛选和整体布局。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    AdminMailIntakeQuery, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
};

use super::super::{AdminActionFeedback, admin_api_error_message};

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub fn page() -> Element {
    let mut mailbox_key = use_signal(String::new);
    let mut world_filter = use_signal(String::new);
    let mut kind_filter = use_signal(String::new);
    let mut status_filter = use_signal(String::new);
    let mut active_mailbox = use_signal(String::new);
    // 旧版邮件页首次打开显示所有邮箱的入站记录；轮询状态只有输入具体邮箱后才读取。
    let active_query = use_signal(AdminMailIntakeQuery::default);
    let page_number = use_signal(|| 1_i64);
    let refresh = use_signal(|| 0_u64);
    let mut polling_refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut selected_intake = use_signal(|| None::<String>);

    let polling = use_resource(move || {
        let _ = polling_refresh();
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

    let load_polling = move |event: FormEvent| {
        event.prevent_default();
        let next_mailbox = mailbox_key().trim().to_owned();
        if next_mailbox.is_empty() {
            return;
        }
        active_mailbox.set(next_mailbox);
        notice.set(None);
        polling_refresh.set(polling_refresh().saturating_add(1));
    };

    let intake_content = match &*intakes.read_unchecked() {
        None => rsx! {
            p { class: "admin-shell__status", role: "status", {t!("admin-mail-loading")} }
        },
        Some(Err(error)) => rsx! {
            p {
                class: "admin-shell__inline-error",
                role: "alert",
                {admin_api_error_message(error)}
            }
        },
        Some(Ok(intakes)) => rsx! {
            super::table::MailIntakeTable {
                page: intakes.clone(),
                page_number,
                on_select: move |intake_id| selected_intake.set(Some(intake_id)),
            }
        },
    };
    let (detail_value, detail_loading, detail_error) = match &*detail.read_unchecked() {
        None => (None, true, None),
        Some(Err(code)) => (None, false, Some(code.clone())),
        Some(Ok(value)) => (value.clone(), false, None),
    };
    let polling_content = if active_mailbox().is_empty() {
        rsx! {
            p { {t!("admin-mail-polling-not-loaded")} }
        }
    } else {
        match &*polling.read_unchecked() {
            None => rsx! {
                p { class: "admin-panel__note", role: "status", {t!("admin-mail-loading")} }
            },
            Some(Err(error)) => rsx! {
                p {
                    class: "admin-panel__note admin-panel__note--error",
                    role: "alert",
                    {admin_api_error_message(error)}
                }
            },
            Some(Ok(Some(polling))) => rsx! {
                super::actions::MailPollingCard {
                    polling: polling.clone(),
                    notice,
                    refresh: polling_refresh,
                }
            },
            Some(Ok(None)) => rsx! {
                p { {t!("admin-mail-polling-not-loaded")} }
            },
        }
    };

    rsx! {
        section { class: "admin-mail-page", aria_labelledby: "mail-title",
                    h1 { id: "mail-title", {t!("page-mails-title")} }
                    div { class: "admin-mail-filters",
                        label {
                            r#for: "mail-world",
                            {t!("admin-mail-intake-world")}
                            select {
                                id: "mail-world",
                                class: "admin-mail-world",
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
                                            mailbox_key: String::new(),
                                            world_id,
                                            kind: kind_filter().trim().to_owned(),
                                            status: status_filter().trim().to_owned(),
                                            decision: String::new(),
                                            received_from: String::new(),
                                            received_to: String::new(),
                                        },
                                    );
                                },
                                    option { value: "", {t!("admin-mail-all")} }
                                option { value: "main", {t!("admin-mail-filter-world-main")} }
                                option { value: "isekai", {t!("admin-mail-filter-world-isekai")} }
                            }
                        }
                        label {
                            r#for: "mail-status",
                            {t!("admin-mail-intake-status")}
                            select {
                                id: "mail-status",
                                class: "admin-mail-state-filter",
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
                                            mailbox_key: String::new(),
                                            world_id: world_filter().trim().to_owned(),
                                            kind: kind_filter().trim().to_owned(),
                                            status,
                                            decision: String::new(),
                                            received_from: String::new(),
                                            received_to: String::new(),
                                        },
                                    );
                                },
                                option { value: "", {t!("admin-mail-all")} }
                                option { value: "accepted", {t!("admin-mail-filter-status-accepted")} }
                                option { value: "rejected", {t!("admin-mail-filter-status-rejected")} }
                                option { value: "failed", {t!("admin-mail-filter-status-failed")} }
                            }
                        }
                        label {
                            r#for: "mail-kind",
                            {t!("admin-mail-intake-kind")}
                            select {
                                id: "mail-kind",
                                class: "admin-mail-kind",
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
                                            mailbox_key: String::new(),
                                            world_id: world_filter().trim().to_owned(),
                                            kind,
                                            status: status_filter().trim().to_owned(),
                                            decision: String::new(),
                                            received_from: String::new(),
                                            received_to: String::new(),
                                        },
                                    );
                                },
                                option { value: "", {t!("admin-mail-all")} }
                                option { value: "equipment", {t!("admin-mail-filter-kind-equipment")} }
                                option { value: "bid", {t!("admin-mail-filter-kind-bid")} }
                                option { value: "delivery", {t!("admin-mail-filter-kind-delivery")} }
                                option { value: "registration", {t!("admin-mail-filter-kind-registration")} }
                                option { value: "rebind", {t!("admin-mail-filter-kind-rebind")} }
                                option { value: "unknown", {t!("admin-mail-filter-kind-unknown")} }
                            }
                        }
                    }
                    {intake_content}
                    super::detail::MailIntakeDetailPanel {
                        visible: selected_intake().is_some(),
                        detail: detail_value,
                        loading: detail_loading,
                        error: detail_error,
                        refresh,
                        on_close: move |_| selected_intake.set(None),
                    }
                    details { class: "admin-mail-polling",
                        summary { id: "mail-polling-title", {t!("admin-mail-polling-title")} }
                        form {
                            class: "admin-mail-search",
                            onsubmit: load_polling,
                            label { r#for: "mailbox-key", {t!("admin-mail-mailbox")} }
                            input {
                                id: "mailbox-key",
                                maxlength: "256",
                                required: true,
                                value: mailbox_key(),
                                oninput: move |event| mailbox_key.set(event.value()),
                            }
                            button { r#type: "submit", {t!("admin-mail-load")} }
                        }
                        super::actions::MailTriggerButton { notice, refresh: polling_refresh }
                        {polling_content}
                        if let Some(feedback) = notice() {
                            p {
                                class: if feedback.is_success() {
                                    "admin-panel__note action-feedback action-feedback--success"
                                } else {
                                    "admin-panel__note action-feedback action-feedback--error"
                                },
                                role: if feedback.is_success() { "status" } else { "alert" },
                                {feedback.message(t!("admin-mail-action-success"))}
                            }
                        }
                    }
        }
    }
}

/// 让旧版的闭集筛选在选择变化后立即生效，同时清理详情和操作反馈。
pub(super) fn activate_mail_query(
    mut active_query: Signal<AdminMailIntakeQuery>,
    mut page_number: Signal<i64>,
    mut selected_intake: Signal<Option<String>>,
    mut notice: Signal<Option<AdminActionFeedback>>,
    mut refresh: Signal<u64>,
    query: AdminMailIntakeQuery,
) {
    active_query.set(query);
    page_number.set(1);
    selected_intake.set(None);
    notice.set(None);
    refresh.set(refresh().saturating_add(1));
}
