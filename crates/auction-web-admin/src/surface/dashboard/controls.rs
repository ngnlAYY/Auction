//! Admin Dashboard 邮件轮询控制。
//! 命令由认证会话持有，后台快照失败卸载控件时仍保留原参数与 UUID；
//! 暂停冲突会刷新唯一 Dashboard 快照，但不会把 stale 结果伪装成成功。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::surface::mail::{MAILBOX_ENTITY_PREFIX, mailbox_command_entity};
use crate::surface::pending::{
    CommandDelivery, CommandReceipt, PendingCommands, set_if_alive, try_set_if_alive,
};

#[component]
pub(super) fn MailPollerControls(
    pollers: Vec<AdminMailPolling>,
    refresh: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let owner = use_context::<PendingCommands>();
    let mut notice = use_signal(|| None::<String>);
    let mut error_code = use_signal(|| None::<String>);
    // 暂停与恢复携带邮箱版本，快照被命令失效后先读同一邮箱的新版本。
    let read_is_current = move || refresh() == loaded_generation;
    // 按钮文案在 rsx 外配好，循环体根节点才能直接是按邮箱带 key 的按钮。
    let poller_buttons = pollers.into_iter().map(|polling| {
        let action = if polling.paused {
            t!("dashboard-legacy-resume-poll")
        } else {
            t!("dashboard-legacy-pause-poll")
        };
        let label = t!(
            "dashboard-legacy-poller-action",
            action: action,
            mailbox: polling.mailbox_key.clone()
        );
        let entity = mailbox_command_entity(&polling.mailbox_key);
        (polling, label, entity)
    });

    rsx! {
        Fragment {
            div { class: "admin-dashboard-actions",
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: owner.contains("mail-poll-trigger"),
                    onclick: move |_| {
                        if owner.contains("mail-poll-trigger") {
                            return;
                        }
                        notice.set(None);
                        error_code.set(None);
                        owner.submit_with_receipt(
                            "mail-poll-trigger".to_owned(),
                            t!("dashboard-legacy-trigger-poll"),
                            |command_id| async move {
                                trigger_mail_poll(command_id).await.map_err(|error| error.code)
                            },
                            move |count, source_alive| {
                                let message = t!("dashboard-legacy-poll-triggered", count: count);
                                CommandDelivery::reported(source_alive && try_set_if_alive(notice, Some(message.clone())), || Some(CommandReceipt::Text(message)))
                            },
                            move |code| set_if_alive(error_code, Some(code)),
                        );
                    },
                    {t!("dashboard-legacy-trigger-poll")}
                }
                for (polling, label, entity) in poller_buttons {
                    button {
                        class: "admin-btn-secondary",
                        key: "{polling.mailbox_key}",
                        r#type: "button",
                        disabled: !read_is_current() || owner.contains(&entity) || owner.contains_local(MAILBOX_ENTITY_PREFIX),
                        onclick: move |_| {
                            if !read_is_current() || owner.contains(&entity) || owner.contains_local(MAILBOX_ENTITY_PREFIX) {
                                return;
                            }
                            notice.set(None);
                            error_code.set(None);
                            let polling = polling.clone();
                            let desired_paused = !polling.paused;
                            // 保留本页暂停/恢复串行策略；未知结果必须先原样解决。
                            owner.submit_with_receipt(
                                entity.clone(),
                                label.clone(),
                                move |command_id| {
                                    let polling = polling.clone();
                                    async move {
                                        let result = set_mail_paused(command_id, polling, desired_paused).await;
                                        let mut refresh = refresh;
                                        if result.as_ref().is_err_and(crate::api::AdminApiFailure::is_conflict)
                                            && let Ok(mut generation) = refresh.try_write()
                                        {
                                            *generation = generation.saturating_add(1);
                                        }
                                        result.map_err(|error| error.code)
                                    }
                                },
                                move |_, source_alive| CommandDelivery::reported(source_alive && try_set_if_alive(notice, Some(t!("dashboard-legacy-poll-updated"))), || None),
                                move |code| set_if_alive(error_code, Some(code)),
                            );
                        },
                        "{label}"
                    }
                }
            }
            if let Some(code) = error_code() {
                p { class: "action-feedback action-feedback--error", role: "alert",
                    {super::super::admin_api_error_message(&code)}
                }
            }
            if let Some(message) = notice() {
                p { class: "action-feedback action-feedback--success", role: "status", "{message}" }
            }
        }
    }
}
