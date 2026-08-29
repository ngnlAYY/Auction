//! Admin 邮件轮询触发和暂停/恢复操作。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};

use super::super::{AdminActionFeedback, format_admin_time};

#[component]
pub(super) fn MailTriggerButton(
    mut notice: Signal<Option<AdminActionFeedback>>,
    mut refresh: Signal<u64>,
) -> Element {
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
                            notice.set(Some(AdminActionFeedback::Success));
                            command_state.set(None);
                            refresh.set(refresh().saturating_add(1));
                        }
                        Err(error) => {
                            notice.set(Some(AdminActionFeedback::Error(error.code)))
                        }
                    }
                    busy.set(false);
                });
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
    let mut busy = use_signal(|| false);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let pause_polling = polling.clone();
    let pause_notice = notice;
    let pause_refresh = refresh;
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
                                notice.set(Some(AdminActionFeedback::Success));
                                command_state.set(None);
                                refresh.set(refresh().saturating_add(1));
                            }
                            Err(error) => {
                                notice.set(Some(AdminActionFeedback::Error(error.code)))
                            }
                        }
                        busy.set(false);
                    });
                },
                if busy() { {t!("admin-action-working")} } else { "{label}" }
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
