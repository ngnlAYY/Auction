//! Admin 邮件轮询触发和暂停/恢复操作。

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::surface::pending::{
    PendingCommandPanel, PendingCommands, set_if_alive, use_pending_commands,
};

use super::super::{AdminActionFeedback, format_admin_time};
use super::progress::MailPollingProgress;

/// 轮询卡片可以随邮箱切换，命令宿主始终保留；成功只刷新轮询状态，不干扰入站投递。
#[component]
pub(super) fn MailPollingCommands(refresh: Signal<u64>, children: Element) -> Element {
    let owner = use_pending_commands(refresh);
    rsx! {
        PendingCommandPanel { owner }
        {children}
    }
}

#[component]
pub(super) fn MailTriggerButton(mut notice: Signal<Option<AdminActionFeedback>>) -> Element {
    let owner = use_context::<PendingCommands>();
    let busy = owner.is_busy("mail-poll-trigger");
    rsx! {
        button {
            class: "admin-btn-primary",
            r#type: "button",
            disabled: owner.contains("mail-poll-trigger"),
            onclick: move |_| {
                notice.set(None);
                owner.submit(
                    "mail-poll-trigger".to_owned(),
                    t!("admin-mail-trigger"),
                    |command_id| async move {
                        trigger_mail_poll(command_id).await.map_err(|error| error.code)
                    },
                    move |_| set_if_alive(notice, Some(AdminActionFeedback::Success)),
                    move |code| set_if_alive(notice, Some(AdminActionFeedback::Error(code))),
                );
            },
            if busy { {t!("admin-action-working")} } else { {t!("admin-mail-trigger")} }
        }
    }
}

#[component]
pub(super) fn MailPollingCard(polling: AdminMailPolling) -> Element {
    // 反馈属于按邮箱重挂载的卡片；旧命令仍由稳定宿主持有，不能写到后来选择的邮箱。
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let desired_paused = !polling.paused;
    let label = if polling.paused { t!("admin-mail-resume") } else { t!("admin-mail-pause") };
    let state_label =
        if polling.paused { t!("admin-mail-paused") } else { t!("admin-mail-running") };
    let owner = use_context::<PendingCommands>();
    let entity = format!("mailbox:{}", polling.mailbox_key);
    let busy = owner.is_busy(&entity);
    let blocked = owner.contains(&entity);
    let pause_polling = polling.clone();
    let action_label = label.clone();
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
                disabled: blocked,
                onclick: move |_| {
                    notice.set(None);
                    // 卡片随邮箱切换卸载，已提交暂停/恢复仍由页级 owner 保留原负载和结果。
                    let polling = pause_polling.clone();
                    owner.submit(
                        entity.clone(),
                        action_label.clone(),
                        move |command_id| {
                            let polling = polling.clone();
                            async move {
                                set_mail_paused(command_id, polling, desired_paused)
                                    .await
                                    .map_err(|error| error.code)
                            }
                        },
                        move |_| set_if_alive(notice, Some(AdminActionFeedback::Success)),
                        move |code| set_if_alive(notice, Some(AdminActionFeedback::Error(code))),
                    );
                },
                if busy { {t!("admin-action-working")} } else { "{label}" }
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
