//! Admin 邮件页面加载、筛选和整体布局。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind, QueryState};

use crate::api::{fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state};

use super::super::query::{MailQuery, use_mail_query_state};
use super::super::{AdminActionFeedback, admin_api_error_message};
use crate::surface::pagination::{use_admin_page, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub fn page() -> Element {
    // 入站列表的世界/类型/状态筛选与分页跟地址栏双向同步，刷新/分享/前进后退可恢复；
    // 邮箱轮询输入、详情面板开合和操作反馈是瞬态，仍由 signal 持有、不入 URL。
    let query = use_mail_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.to_intake_query(), current.page)
    });
    let mut mailbox_key = use_signal(String::new);
    let mut active_mailbox = use_signal(String::new);
    let mut refresh = use_signal(|| 0_u64);
    let delivery_commands = use_pending_commands(refresh);
    let mut polling_refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    // 重排反馈由页面持有，详情刷新或切换期间不会把成功排队误报为已送达。
    let mut delivery_notice = use_signal(|| None::<(String, AdminActionFeedback)>);
    let mut selected_intake = use_signal(|| None::<String>);

    let polling = use_resource(move || {
        let _ = polling_refresh();
        let mailbox = active_mailbox();
        async move {
            let result = if mailbox.is_empty() {
                Ok(None)
            } else {
                fetch_mail_state(mailbox.clone()).await.map(Some)
            };
            (mailbox, result)
        }
    });
    let pagination = use_admin_pagination("mail", &["page_size"], Some(100), i64::from(i32::MAX));
    let intakes = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |(query, page), size| fetch_mail_intakes(query, page, size),
    );
    let detail = use_resource(move || {
        let _ = refresh();
        let intake_id = selected_intake();
        async move {
            let result = match intake_id.as_ref() {
                Some(intake_id) => {
                    fetch_mail_intake_detail(intake_id.clone()).await.and_then(|detail| {
                        if &detail.id == intake_id {
                            Ok(Some(detail))
                        } else {
                            Err("invalid_response".to_owned())
                        }
                    })
                }
                None => Ok(None),
            };
            (intake_id, result)
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

    // 三个闭集筛选各持一份 hook 克隆；克隆共享同一状态信号。
    let mut world_query = query.clone();
    let mut status_query = query.clone();
    let mut kind_query = query.clone();

    let intake_content = match &*intakes.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-mail-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(intakes)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query.clone();
            rsx! {
                super::table::MailIntakeTable {
                    page: intakes.clone(),
                    pagination,
                    sort: query_state().sort.clone(),
                    direction: query_state().direction,
                    on_page_change: move |next_page| {
                        let mut next = query_state();
                        next.page = next_page;
                        on_page_change.push(next);
                    },
                    on_sort_change: move |(sort, direction)| {
                        let mut next = query_state();
                        next.sort = sort;
                        next.direction = direction;
                        on_sort_change.push(next);
                    },
                    on_select: move |intake_id| {
                        delivery_notice.set(None);
                        selected_intake.set(Some(intake_id));
                    },
                }
            }
        }
    };
    // Resource 重启会保留旧值；只有同一收件的快照可继续展示和操作。
    let detail_state = detail.read_unchecked();
    let current_detail = detail_state
        .as_ref()
        .and_then(|(intake_id, result)| (intake_id == &selected_intake()).then_some(result));
    let (detail_value, detail_loading, detail_error) = match current_detail {
        None => (None, true, None),
        Some(Err(code)) => (None, false, Some(code.clone())),
        Some(Ok(value)) => (value.clone(), false, None),
    };
    let polling_state = polling.read_unchecked();
    // 只保留当前邮箱的完成快照；切换邮箱不能继续显示或操作上一邮箱的轮询状态。
    let current_polling = polling_state
        .as_ref()
        .and_then(|(mailbox, result)| (mailbox == &active_mailbox()).then_some(result));
    let polling_content = if active_mailbox().is_empty() {
        rsx! {
            p { {t!("admin-mail-polling-not-loaded")} }
        }
    } else {
        match current_polling {
            None => rsx! {
                AsyncState {
                    kind: AsyncStateKind::Loading,
                    title: t!("admin-mail-loading"),
                    detail: "",
                }
            },
            Some(Err(error)) => rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: admin_api_error_message(error),
                    detail: "",
                    retry_label: t!("admin-retry"),
                    on_retry: move |_| {
                        polling_refresh.set(polling_refresh().saturating_add(1));
                    },
                }
            },
            Some(Ok(Some(polling))) => rsx! {
                for polling in [polling.clone()] {
                    super::actions::MailPollingCard {
                        key: "{polling.mailbox_key}",
                        polling,
                    }
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
                    PendingCommandPanel { owner: delivery_commands }
                    div { class: "admin-mail-filters",
                        label {
                            r#for: "mail-world",
                            {t!("admin-mail-intake-world")}
                            select {
                                id: "mail-world",
                                name: "world",
                                autocomplete: "off",
                                class: "admin-mail-world",
                                value: query_state().world,
                                onchange: move |event| {
                                    apply_mail_filter(
                                        &mut world_query,
                                        selected_intake,
                                        notice,
                                        |next| next.world = event.value(),
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
                                name: "status",
                                autocomplete: "off",
                                class: "admin-mail-state-filter",
                                value: query_state().status,
                                onchange: move |event| {
                                    apply_mail_filter(
                                        &mut status_query,
                                        selected_intake,
                                        notice,
                                        |next| next.status = event.value(),
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
                                name: "kind",
                                autocomplete: "off",
                                class: "admin-mail-kind",
                                value: query_state().kind,
                                onchange: move |event| {
                                    apply_mail_filter(
                                        &mut kind_query,
                                        selected_intake,
                                        notice,
                                        |next| next.kind = event.value(),
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
                        delivery_notice,
                        on_close: move |_| {
                            delivery_notice.set(None);
                            selected_intake.set(None);
                        },
                    }
                    if let Some((intake_id, feedback)) = delivery_notice()
                        && selected_intake().as_ref() == Some(&intake_id)
                    {
                        p {
                            class: if feedback.is_success() {
                                "admin-panel__note action-feedback action-feedback--success"
                            } else {
                                "admin-panel__note action-feedback action-feedback--error"
                            },
                            role: if feedback.is_success() { "status" } else { "alert" },
                            {feedback.message(t!("admin-mail-delivery-requeued"))}
                        }
                    }
                    details { class: "admin-mail-polling",
                        summary { id: "mail-polling-title", {t!("admin-mail-polling-title")} }
                        form {
                            class: "admin-mail-search",
                            onsubmit: load_polling,
                            label { r#for: "mailbox-key", {t!("admin-mail-mailbox")} }
                            input {
                                id: "mailbox-key",
                                name: "mailbox_key",
                                autocomplete: "off",
                                autocapitalize: "none",
                                spellcheck: "false",
                                enterkeyhint: "search",
                                maxlength: "256",
                                required: true,
                                value: mailbox_key(),
                                oninput: move |event| mailbox_key.set(event.value()),
                            }
                            button { r#type: "submit", {t!("admin-mail-load")} }
                        }
                        super::actions::MailPollingCommands {
                            refresh: polling_refresh,
                            super::actions::MailTriggerButton { notice }
                            {polling_content}
                        }
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

/// 闭集筛选选择变化后立即生效：push 到地址栏并回到第一页，同时清理详情与操作反馈——
/// 旧选择不再属于新结果集。查询资源响应 `query.state` 变化自动重新加载，无需手动刷新。
fn apply_mail_filter(
    query: &mut QueryState<MailQuery>,
    mut selected_intake: Signal<Option<String>>,
    mut notice: Signal<Option<AdminActionFeedback>>,
    update: impl FnOnce(&mut MailQuery),
) {
    let state = query.state;
    let mut next = state();
    update(&mut next);
    next.page = 1;
    query.push(next);
    selected_intake.set(None);
    notice.set(None);
}
