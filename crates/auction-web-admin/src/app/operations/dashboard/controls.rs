//! Admin Dashboard 邮件轮询控制。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};

#[component]
pub(super) fn MailPollerControls(
    pollers: Vec<AdminMailPolling>,
    mut refresh: Signal<u64>,
) -> Element {
    let mut busy = use_signal(|| false);
    let notice = use_signal(|| None::<bool>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    rsx! {
        section {
            class: "admin-panel admin-dashboard__mail-controls",
            aria_labelledby: "dashboard-mail-pollers-title",
            h2 { id: "dashboard-mail-pollers-title", {t!("admin-mail-polling-title")} }
            div { class: "admin-panel__actions",
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        if busy() {
                            return;
                        }
                        busy.set(true);
                        let command_id = stable_command_id(command_state, "trigger".to_owned());
                        let mut notice = notice;
                        let mut refresh = refresh;
                        let mut busy = busy;
                        spawn(async move {
                            if trigger_mail_poll(command_id).await.is_ok() {
                                notice.set(Some(true));
                                command_state.set(None);
                                refresh.set(refresh().saturating_add(1));
                            } else {
                                notice.set(Some(false));
                            }
                            busy.set(false);
                        });
                    },
                    {t!("admin-mail-trigger")}
                }
                for polling in pollers.iter().cloned() {
                    {
                        let desired_paused = !polling.paused;
                        let label = if polling.paused {
                            t!("admin-mail-resume")
                        } else {
                            t!("admin-mail-pause")
                        };
                        let mailbox_key = polling.mailbox_key.clone();
                        rsx! {
                            div { class: "admin-dashboard__mail-poller", key: "{mailbox_key}",
                                p { class: "admin-panel__note",
                                    {t!("admin-mail-polling-summary",
                                        mailbox: mailbox_key.clone(),
                                        status: if polling.paused {
                                            t!("admin-mail-paused")
                                        } else {
                                            t!("admin-mail-running")
                                        },
                                        version: polling.state_version,
                                        failures: polling.consecutive_failures
                                    )}
                                }
                                button {
                                    class: "admin-btn-secondary",
                                    r#type: "button",
                                    disabled: busy(),
                                    onclick: move |_| {
                                        if busy() {
                                            return;
                                        }
                                        busy.set(true);
                                        let polling = polling.clone();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("pause:{}:{}:{}", polling.mailbox_key, polling.state_version, desired_paused),
                                        );
                                        let mut notice = notice;
                                        let mut refresh = refresh;
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
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
            if let Some(success) = notice() {
                p {
                    class: if success { "admin-panel__note" } else { "admin-panel__note admin-panel__note--error" },
                    role: "status",
                    {t!(if success { "admin-mail-action-success" } else { "admin-mail-action-error" })}
                }
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
