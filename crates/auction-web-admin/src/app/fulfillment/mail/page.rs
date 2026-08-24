//! Admin 邮件页面加载、筛选和整体布局。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminMailIntakeQuery, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
};

use super::super::{AdminLoadError, AdminPanel};

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub fn page() -> Element {
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
                        super::actions::MailTriggerButton { notice, refresh }
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
                        super::actions::MailPollingCard { polling, notice, refresh }
                    } else {
                        p { class: "admin-panel__note", {t!("admin-mail-polling-not-loaded")} }
                    }
                    super::table::MailIntakeTable {
                        page: intakes.clone(),
                        page_number,
                        on_select: move |intake_id| selected_intake.set(Some(intake_id)),
                    }
                    super::detail::MailIntakeDetailPanel {
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
pub(super) fn activate_mail_query(
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
