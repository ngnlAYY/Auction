//! Admin 邮件轮询触发和暂停/恢复操作。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::super::{admin_api_error_message, format_admin_time};
use super::mailbox_command_entity;
use super::progress::MailPollingProgress;
use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommandPanel, PendingCommands, set_if_alive,
    try_set_if_alive, use_pending_commands,
};

/// 轮询卡片可以随邮箱切换，命令宿主始终保留；成功只刷新轮询状态，不干扰入站投递。
#[component]
pub(super) fn MailPollingCommands(
    refresh: Signal<u64>,
    active_mailbox: Signal<String>,
    children: Element,
) -> Element {
    let owner = use_pending_commands(refresh, move |entity| {
        entity == "mail-poll-trigger"
            || entity
                .strip_prefix(super::MAILBOX_ENTITY_PREFIX)
                .is_some_and(|mailbox| mailbox == active_mailbox.peek().as_str())
    });
    rsx! {
        PendingCommandPanel { owner }
        {children}
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(super) struct PollingReply(
    pub(super) std::rc::Rc<crate::surface::auctions::editor::tests::RecordRead<AdminMailPolling>>,
);

#[component]
pub(super) fn MailTriggerButton() -> Element {
    let owner = use_context::<PendingCommands>();
    let busy = owner.is_busy("mail-poll-trigger");
    let mut notice = use_signal(|| None::<Result<i64, String>>);
    rsx! {
        button {
            class: "admin-btn-primary",
            r#type: "button",
            disabled: owner.contains("mail-poll-trigger"),
            onclick: move |_| {
                if owner.contains("mail-poll-trigger") { return; }
                notice.set(None);
                owner.submit_with_receipt(
                    "mail-poll-trigger".to_owned(),
                    t!("admin-mail-trigger"),
                    |command_id| async move {
                        trigger_mail_poll(command_id).await
                    },
                    move |pollers, source_alive| CommandDelivery::reported(
                        source_alive && try_set_if_alive(notice, Some(Ok(pollers))),
                        || Some(CommandReceipt::Text(t!("admin-mail-trigger-accepted", pollers: pollers))),
                    ),
                    move |code| set_if_alive(notice, Some(Err(code))),
                );
            },
            if busy { {t!("admin-action-working")} } else { {t!("admin-mail-trigger")} }
        }
        div { role: "status",
            if let Some(Ok(pollers)) = notice() {
                p { class: "admin-panel__note action-feedback action-feedback--success",
                    {t!("admin-mail-trigger-accepted", pollers: pollers)}
                }
            }
        }
        if let Some(Err(code)) = notice() {
            p { class: "admin-panel__note action-feedback action-feedback--error", role: "alert",
                {admin_api_error_message(&code)}
            }
        }
    }
}

#[component]
pub(super) fn MailPollingCard(
    polling: AdminMailPolling,
    refresh: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let mut error_code = use_signal(|| None::<String>);
    let desired_paused = !polling.paused;
    let label = if polling.paused { t!("admin-mail-resume") } else { t!("admin-mail-pause") };
    let state_label =
        if polling.paused { t!("admin-mail-paused") } else { t!("admin-mail-running") };
    let owner = use_context::<PendingCommands>();
    let entity = mailbox_command_entity(&polling.mailbox_key);
    let busy = owner.is_busy(&entity);
    // 切换回来也须等命令触发的 GET，不能按刚刚失效的邮箱版本分配新 UUID。
    let read_is_current = move || refresh() == loaded_generation;
    let blocked = owner.contains(&entity);
    let button_label = if busy { t!("admin-action-working") } else { label.clone() };
    let pause_polling = polling.clone();
    rsx! {
        article {
            class: "admin-mail-state",
            aria_busy: busy.to_string(),
            h3 { {polling.mailbox_key.clone()} }
            dl {
                div {
                    dt { {t!("admin-mail-polling-state")} }
                    dd { "{state_label}" }
                }
                div {
                    dt { {t!("admin-mail-polling-failures")} }
                    dd { "{polling.consecutive_failures}" }
                }
                div {
                    dt { {t!("admin-mail-polling-last")} }
                    dd { {format_admin_time(&polling.last_poll_at)} }
                }
                div {
                    dt { {t!("admin-mail-polling-next")} }
                    dd { {format_admin_time(&polling.next_poll_at)} }
                }
            }
            MailPollingProgress { diagnostics: polling.diagnostics }
            button {
                r#type: "button",
                disabled: blocked || !read_is_current(),
                onclick: move |_| {
                    if !read_is_current() || owner.contains(&entity) { return; }
                    error_code.set(None);
                    // 卡片随邮箱切换卸载，已提交暂停/恢复仍由会话宿主保留原负载和结果。
                    let polling = pause_polling.clone();
                    owner.submit_with_receipt(
                        entity.clone(),
                        label.clone(),
                        move |command_id| {
                            let polling = polling.clone();
                            async move {
                                #[cfg(test)]
                                if let Some(reply) = try_consume_context::<PollingReply>() {
                                    return reply.0.fetch().await.map_err(crate::api::AdminWriteFailure::outcome_unknown);
                                }
                                set_mail_paused(command_id, polling, desired_paused)
                                    .await
                            }
                        },
                        move |_, source_alive| {
                            if source_alive {
                                set_if_alive(error_code, None);
                            }
                            // 成功与邮箱绑定，由会话交付，卡片读取失败或切换邮箱都可以卸载卡片。
                            CommandDelivery::Receipt(Some(CommandReceipt::Text(t!("admin-mail-action-success"))))
                        },
                        move |code| set_if_alive(error_code, Some(code)),
                    );
                },
                "{button_label}"
            }
            if let Some(code) = error_code() {
                p {
                    class: "admin-panel__note action-feedback action-feedback--error",
                    role: "alert",
                    {admin_api_error_message(&code)}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use dioxus::core::{AttributeValue, Mutation};
    use dioxus::prelude::*;
    use gloo_net::http::Method;

    use super::{MailPollingCard, MailPollingCommands, MailTriggerButton, PollingReply};
    use crate::api::{AdminApiFailure, AdminMailPolling};
    use crate::surface::auctions::editor::tests::{
        RecordRead, button, click_button, has_live_text, init_i18n, mount_writer, settle_writer,
    };
    use crate::surface::pending::{PendingCommandHost, PendingCommandNotices};

    fn polling(mailbox_key: String) -> AdminMailPolling {
        AdminMailPolling {
            mailbox_key,
            paused: false,
            state_version: 1,
            consecutive_failures: 0,
            last_poll_at: "2026-11-18T00:00:00Z".to_owned(),
            next_poll_at: "2026-11-18T00:01:00Z".to_owned(),
            updated_at: "2026-11-18T00:00:00Z".to_owned(),
            diagnostics: None,
        }
    }

    #[test]
    fn mailbox_switch_keeps_the_first_mailbox_receipt_and_does_not_reload_the_other_mailbox() {
        for (switch, initially_paused, valid) in
            [(false, false, true), (true, false, true), (true, true, true), (true, false, false)]
        {
            let reply = Rc::new(RecordRead::new());
            let (mut page, _) = mount_writer({
                let reply = reply.clone();
                move || {
                    init_i18n();
                    use_context_provider(|| PollingReply(reply.clone()));
                    let host = use_context::<PendingCommandHost>();
                    let reload = use_signal(|| 0);
                    let mut mailbox = use_signal(|| "mail-a".to_owned());
                    rsx! {
                        PendingCommandNotices { host }
                        button { onclick: move |_| mailbox.set("mail-b".to_owned()), {"切换邮箱".to_owned()} }
                        p { "轮询世代 {reload()}" }
                        MailPollingCommands { refresh: reload, active_mailbox: mailbox,
                            for key in [mailbox()] {
                                MailPollingCard {
                                    key: "{key}",
                                    polling: AdminMailPolling { paused: initially_paused, ..polling(key) },
                                    refresh: reload,
                                    loaded_generation: 0,
                                }
                            }
                        }
                    }
                }
            });
            let label = if initially_paused { "恢复轮询" } else { "暂停轮询" };
            click_button(&mut page, label);
            if switch {
                click_button(&mut page, "切换邮箱");
            }
            let mut accepted = polling("mail-a".to_owned());
            accepted.paused = !initially_paused;
            accepted.state_version = 2;
            reply.respond(if valid { Ok(accepted) } else { Err("invalid_response".to_owned()) });
            settle_writer(&mut page);
            if !valid {
                assert!(button(&page, "关闭回执").is_none());
                assert!(!has_live_text(&page, "邮件轮询状态已更新。"));
                assert!(has_live_text(&page, "轮询世代 0"));
                assert!(!button(&page, "原样重试").unwrap().1);
                continue;
            }
            assert!(button(&page, "关闭回执").is_some());
            assert!(has_live_text(&page, "邮件轮询状态已更新。"));
            if switch {
                assert!(has_live_text(&page, "轮询世代 0"));
                assert!(!button(&page, label).unwrap().1);
            } else {
                assert!(has_live_text(&page, "轮询世代 1"));
                let (pause, disabled) = button(&page, "暂停轮询").unwrap();
                assert!(disabled);
                page.dispatch("click", pause, "");
                assert!(has_live_text(&page, "轮询世代 1"));
                assert!(has_live_text(&page, "邮件轮询状态已更新。"));
            }
        }
    }

    #[test]
    fn mail_trigger_with_unreadable_http_reply_keeps_the_original_intent_pending() {
        let (mut page, initial) = mount_writer(|| {
            init_i18n();
            AdminApiFailure::stub_write_response(
                "/api/v1/admin/mail/polls:trigger",
                Method::POST,
                503,
                "{",
            );
            let refresh = use_signal(|| 0);
            let active_mailbox = use_signal(String::new);
            rsx! {
                MailPollingCommands { refresh, active_mailbox,
                    MailTriggerButton {}
                }
            }
        });
        assert!(has_live_text(&page, "唤醒全部邮件轮询器"));
        // 按钮的两种文案经条件片段渲染；从挂载时的真实监听定位触发按钮。
        let trigger = initial
            .iter()
            .find_map(|mutation| match mutation {
                Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
                _ => None,
            })
            .unwrap();
        let mut edits = page.dispatch("click", trigger, "");
        edits.extend(settle_writer(&mut page));
        assert!(edits.iter().any(|mutation| matches!(
            mutation,
            Mutation::SetAttribute { id, name: "disabled", value: AttributeValue::Bool(true), .. } if *id == trigger
        )));
        assert!(!button(&page, "原样重试").unwrap().1);
        assert!(!has_live_text(&page, "请求已接受，将唤醒 3 个轮询器。"));
    }
}
