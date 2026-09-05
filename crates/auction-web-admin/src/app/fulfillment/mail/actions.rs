//! Admin 邮件轮询触发和暂停/恢复操作。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::app::surface::{run_admin_feedback_action, stable_command_id};

use super::super::{AdminActionFeedback, format_admin_time};

#[component]
pub(super) fn MailTriggerButton(
    mut notice: Signal<Option<AdminActionFeedback>>,
    mut refresh: Signal<u64>,
) -> Element {
    let busy = use_signal(|| false);
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
                let command_id = stable_command_id(command_state, "trigger".to_owned());
                run_admin_feedback_action(
                    busy,
                    notice,
                    async move { trigger_mail_poll(command_id).await.map_err(|error| error.code) },
                    move |_| {
                        command_state.set(None);
                        refresh.set(refresh().saturating_add(1));
                    },
                );
            },
            if busy() { {t!("admin-action-working")} } else { {t!("admin-mail-trigger")} }
        }
    }
}

#[component]
pub(super) fn MailPollingCard(
    polling: AdminMailPolling,
    mut notice: Signal<Option<AdminActionFeedback>>,
    mut refresh: Signal<u64>,
) -> Element {
    let desired_paused = !polling.paused;
    let label = if polling.paused { t!("admin-mail-resume") } else { t!("admin-mail-pause") };
    let state_label =
        if polling.paused { t!("admin-mail-paused") } else { t!("admin-mail-running") };
    let busy = use_signal(|| false);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let pause_polling = polling.clone();
    rsx! {
        article {
            class: "admin-mail-state",
            aria_busy: busy().to_string(),
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
            button {
                r#type: "button",
                disabled: busy(),
                onclick: move |_| {
                    if busy() {
                        return;
                    }
                    let polling = pause_polling.clone();
                    let command_id = stable_command_id(
                        command_state,
                        format!("pause:{}:{}:{}", polling.mailbox_key, polling.state_version, desired_paused),
                    );
                    run_admin_feedback_action(
                        busy,
                        notice,
                        async move {
                            set_mail_paused(command_id, polling, desired_paused)
                                .await
                                .map_err(|error| error.code)
                        },
                        move |_| {
                            command_state.set(None);
                            refresh.set(refresh().saturating_add(1));
                        },
                    );
                },
                if busy() { {t!("admin-action-working")} } else { "{label}" }
            }
        }
    }
}
