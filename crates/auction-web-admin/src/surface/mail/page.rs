//! Admin 邮件页面加载、筛选和整体布局。

#[cfg(test)]
use std::rc::Rc;

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind, QueryState};
use dioxus::prelude::*;

use super::super::deliveries::labels::action_feedback_message;
use super::super::query::{MailQuery, use_mail_query_state};
use super::super::{admin_api_error_message, admin_world_label};
use super::deliveries::MailDeliveryNotices;
#[cfg(test)]
use crate::api::{AdminMailIntakePage, AdminMailPolling};
use crate::api::{
    AdminMailIntakeQuery, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
};
#[cfg(test)]
use crate::surface::auctions::draft_page_tests::RecordRead;
use crate::surface::pagination::{use_admin_page, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub fn page() -> Element {
    // 入站列表的世界/类型/状态筛选与分页跟地址栏双向同步，刷新/分享/前进后退可恢复；
    // 邮箱轮询输入、详情面板开合和操作反馈是瞬态，仍由 signal 持有、不入 URL。
    let query = use_mail_query_state();
    let query_state = query.state;
    let intake_filters = use_memo(move || query_state.read().to_intake_query());
    let requested_query = use_memo(move || (intake_filters(), query_state.read().page));
    let mut mailbox_key = use_signal(String::new);
    let mut active_mailbox = use_signal(String::new);
    let mut refresh = use_signal(|| 0_u64);
    let mut polling_refresh = use_signal(|| 0_u64);
    // 重排反馈按投递归属保存，切换收件时迟到回执不能覆盖其他投递的结果。
    let mut delivery_notices = use_signal(MailDeliveryNotices::new);
    let (mut selected_intake, active_intake) = use_intake_selection(intake_filters);

    let polling = use_resource(move || {
        let generation = polling_refresh();
        let mailbox = active_mailbox();
        async move {
            #[cfg(test)]
            if !mailbox.is_empty()
                && let Some(reads) = try_consume_context::<Rc<RecordRead<AdminMailPolling>>>()
            {
                return (mailbox, generation, reads.fetch().await.map(Some));
            }
            let result = if mailbox.is_empty() {
                Ok(None)
            } else {
                fetch_mail_state(mailbox.clone()).await.map(Some)
            };
            (mailbox, generation, result)
        }
    });
    let pagination = use_admin_pagination("mail", &["page_size"], Some(100), i64::from(i32::MAX));
    let intakes = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |(query, page), size| async move {
            #[cfg(test)]
            if let Some(intakes) = try_consume_context::<AdminMailIntakePage>() {
                return Ok(intakes);
            }
            fetch_mail_intakes(query, page, size).await
        },
    );
    let detail = use_resource(move || {
        let _ = refresh();
        let intake_id = active_intake();
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
    let delivery_commands = use_pending_commands(refresh, move |entity| {
        let Some(delivery_id) = entity.strip_prefix("delivery:") else {
            return false;
        };
        let selected = selected_intake.peek();
        let Some((intake_id, _)) = selected
            .as_ref()
            .filter(|(_, opened_with)| opened_with == &query_state.peek().to_intake_query())
        else {
            return false;
        };
        detail
            .peek()
            .as_ref()
            .filter(|(loaded_id, _)| loaded_id.as_ref() == Some(intake_id))
            .and_then(|(_, result)| result.as_ref().ok())
            .and_then(Option::as_ref)
            .is_none_or(|detail| {
                detail.deliveries.iter().any(|delivery| delivery.id == delivery_id)
            })
    });

    let load_polling = move |event: FormEvent| {
        event.prevent_default();
        let next_mailbox = mailbox_key().trim().to_owned();
        if next_mailbox.is_empty() {
            return;
        }
        active_mailbox.set(next_mailbox);
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
            let mut on_sort_change = query;
            rsx! {
                super::table::MailIntakeTable {
                    page: intakes.clone(),
                    pagination,
                    sort: query_state().sort,
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
                        selected_intake.set(Some((intake_id, query_state.peek().to_intake_query())));
                    },
                }
            }
        }
    };
    // Resource 重启会保留旧值；只有同一收件的快照可继续展示和操作。
    let current_intake = active_intake();
    let detail_state = detail.read_unchecked();
    let current_detail = detail_state
        .as_ref()
        .and_then(|(intake_id, result)| (intake_id == &current_intake).then_some(result));
    let (detail_value, detail_loading, detail_error) = match current_detail {
        None => (None, true, None),
        Some(Err(code)) => (None, false, Some(code.clone())),
        Some(Ok(value)) => (value.clone(), false, None),
    };
    let polling_state = polling.read_unchecked();
    // 只保留当前邮箱的完成快照；切换邮箱不能继续显示或操作上一邮箱的轮询状态。
    let current_polling =
        polling_state.as_ref().filter(|(mailbox, _, _)| mailbox == &active_mailbox());
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
            Some((_, _, Err(error))) => rsx! {
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
            Some((_, loaded_generation, Ok(Some(polling)))) => rsx! {
                for polling in [polling.clone()] {
                    super::actions::MailPollingCard {
                        key: "{polling.mailbox_key}",
                        polling,
                        refresh: polling_refresh,
                        loaded_generation: *loaded_generation,
                    }
                }
            },
            Some((_, _, Ok(None))) => rsx! {
                p { {t!("admin-mail-polling-not-loaded")} }
            },
        }
    };

    let notice_state = delivery_notices.read();
    let current_notices = notice_state
        .iter()
        .filter(|(_, (intake_id, _))| current_intake.as_ref() == Some(intake_id));
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
                                        |next| next.world = event.value(),
                                    );
                                },
                                    option { value: "", {t!("admin-mail-all")} }
                                option { value: "main", {admin_world_label("main")} }
                                option { value: "isekai", {admin_world_label("isekai")} }
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
                                        |next| next.kind = event.value(),
                                    );
                                },
                                option { value: "", {t!("admin-mail-all")} }
                                option { value: "equipment", {t!("admin-mail-filter-kind-equipment")} }
                                option { value: "bid", {t!("admin-mail-filter-kind-bid")} }
                                option { value: "delivery", {t!("admin-mail-filter-kind-delivery")} }
                                option { value: "registration", {t!("admin-mail-filter-kind-registration")} }
                                option { value: "rebind", {t!("admin-mail-filter-kind-rebind")} }
                                option { value: "rekey", {t!("admin-mail-kind-rekey")} }
                                option { value: "unknown", {t!("admin-mail-filter-kind-unknown")} }
                            }
                        }
                    }
                    {intake_content}
                    super::detail::MailIntakeDetailPanel {
                        visible: current_intake.is_some(),
                        detail: detail_value,
                        loading: detail_loading,
                        error: detail_error,
                        refresh,
                        delivery_notices,
                        on_close: move |_| {
                            if let Some((intake_id, _)) = selected_intake.peek().as_ref() {
                                delivery_notices.write().retain(|_, (owner, _)| owner.as_str() != intake_id.as_str());
                            }
                            selected_intake.set(None);
                        },
                    }
                    for (delivery_id, (_, feedback)) in current_notices {
                        p {
                            key: "{delivery_id}",
                            class: if feedback.is_success() {
                                "admin-panel__note action-feedback action-feedback--success"
                            } else {
                                "admin-panel__note action-feedback action-feedback--error"
                            },
                            role: if feedback.is_success() { "status" } else { "alert" },
                            "{delivery_id} · "
                            {action_feedback_message(feedback, t!("admin-mail-delivery-requeued"))}
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
                            active_mailbox,
                            super::actions::MailTriggerButton {}
                            {polling_content}
                        }
                    }
        }
    }
}

type IntakeSelection = (String, AdminMailIntakeQuery);

fn use_intake_selection(
    intake_filters: Memo<AdminMailIntakeQuery>,
) -> (Signal<Option<IntakeSelection>>, Memo<Option<String>>) {
    let mut selected_intake = use_signal(|| None::<IntakeSelection>);
    let active_intake = use_memo(move || {
        let filters = intake_filters.read();
        selected_intake
            .read()
            .as_ref()
            .filter(|(_, opened_with)| opened_with == &*filters)
            .map(|(intake_id, _)| intake_id.clone())
    });
    // 手动筛选与历史恢复都使旧选择失效，返回原筛选也不能重新打开已清除的详情。
    use_effect(move || {
        let filters = intake_filters.read();
        let stale = selected_intake
            .peek()
            .as_ref()
            .is_some_and(|(_, opened_with)| opened_with != &*filters);
        if stale {
            selected_intake.set(None);
        }
    });
    (selected_intake, active_intake)
}

/// 闭集筛选写入地址栏并回到第一页，列表与详情共同响应筛选变化。
fn apply_mail_filter(query: &mut QueryState<MailQuery>, update: impl FnOnce(&mut MailQuery)) {
    let state = query.state;
    let mut next = state();
    update(&mut next);
    next.page = 1;
    query.push(next);
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use auction_contracts::AdminMailPaginationResponse;
    use dioxus::core::Mutation;
    use dioxus::prelude::*;

    use super::{page, use_intake_selection};
    use crate::api::{AdminMailIntakePage, AdminMailIntakeQuery, AdminMailPolling};
    use crate::surface::auctions::draft_page_tests::{
        RecordRead, button, click_button, has_live_text, init_i18n, mount_writer, settle_writer,
        writer_form,
    };
    use crate::surface::mail::actions::PollingReply;
    use crate::surface::pending::{PendingCommandHost, PendingCommandNotices};

    #[test]
    fn intake_selection_stays_closed_after_each_filter_is_restored() {
        const INTAKE_ID: &str = "00000000-0000-4000-8000-000000000022";
        for (dimension, filters) in [
            ("world", AdminMailIntakeQuery { world_id: "isekai".to_owned(), ..Default::default() }),
            ("kind", AdminMailIntakeQuery { kind: "bid".to_owned(), ..Default::default() }),
            ("status", AdminMailIntakeQuery { status: "failed".to_owned(), ..Default::default() }),
        ] {
            let (mut dom, _) = mount_writer(move || {
                let changed = filters.clone();
                let mut query = use_signal(AdminMailIntakeQuery::default);
                let applied = use_memo(move || query.read().clone());
                let (mut selection, active) = use_intake_selection(applied);
                rsx! {
                    button {
                        onclick: move |_| selection.set(Some((INTAKE_ID.to_owned(), applied()))),
                        {"打开收件".to_owned()}
                    }
                    button {
                        onclick: move |_| query.set(changed.clone()),
                        {"改变筛选".to_owned()}
                    }
                    button {
                        onclick: move |_| query.set(AdminMailIntakeQuery::default()),
                        {"恢复筛选".to_owned()}
                    }
                    if let Some(intake_id) = active() {
                        p { "{intake_id}" }
                    }
                }
            });
            click_button(&mut dom, "打开收件");
            assert!(has_live_text(&dom, INTAKE_ID), "{dimension}");
            click_button(&mut dom, "改变筛选");
            assert!(!has_live_text(&dom, INTAKE_ID), "{dimension}");
            click_button(&mut dom, "恢复筛选");
            assert!(!has_live_text(&dom, INTAKE_ID), "{dimension}");
        }
    }

    #[test]
    fn mail_poll_receipt_survives_the_failed_mailbox_read() {
        let reads = Rc::new(RecordRead::new());
        let write = Rc::new(RecordRead::new());
        let (mut dom, edits) = mount_writer({
            let reads = reads.clone();
            let write = write.clone();
            move || {
                init_i18n();
                use_context_provider(|| reads.clone());
                use_context_provider(|| PollingReply(write.clone()));
                use_context_provider(|| AdminMailIntakePage {
                    intakes: Vec::new(),
                    pagination: AdminMailPaginationResponse { page: 1, page_size: 10, total: 0 },
                });
                let host = use_context::<PendingCommandHost>();
                rsx! {
                    PendingCommandNotices { host }
                    {page()}
                }
            }
        });
        let mailbox = edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "input" => Some(*id),
                _ => None,
            })
            .unwrap();
        dom.dispatch("input", mailbox, "mail-a");
        dom.dispatch("submit", writer_form(&edits), "");
        let mut polling = AdminMailPolling {
            mailbox_key: "mail-a".to_owned(),
            paused: false,
            state_version: 1,
            consecutive_failures: 0,
            last_poll_at: "2026-11-18T00:00:00Z".to_owned(),
            next_poll_at: "2026-11-18T00:01:00Z".to_owned(),
            updated_at: "2026-11-18T00:00:00Z".to_owned(),
            diagnostics: None,
        };
        reads.respond(Ok(polling.clone()));
        settle_writer(&mut dom);
        click_button(&mut dom, "暂停轮询");
        polling.paused = true;
        polling.state_version = 2;
        write.respond(Ok(polling.clone()));
        settle_writer(&mut dom);
        let receipt = "邮件轮询状态已更新。";
        assert!(has_live_text(&dom, receipt));
        reads.respond_unavailable();
        settle_writer(&mut dom);
        assert!(button(&dom, "暂停轮询").is_none());
        assert!(has_live_text(&dom, "所需服务暂时不可用。请稍后再试。"));
        assert!(has_live_text(&dom, receipt));
        assert!(button(&dom, "关闭回执").is_some());
        click_button(&mut dom, "重试");
        assert_eq!(reads.started(), 3);
        assert_eq!(write.started(), 1);
        reads.respond(Ok(polling));
        settle_writer(&mut dom);
        assert!(button(&dom, "恢复轮询").is_some());
        assert!(has_live_text(&dom, receipt));
    }
}
