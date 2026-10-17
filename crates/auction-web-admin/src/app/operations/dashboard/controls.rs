//! Admin Dashboard 邮件轮询控制。
//! 控件保持旧版扁平动作区和独立命令 ID owner；暂停冲突会刷新唯一 Dashboard 快照，但不会把
//! stale 结果伪装成成功。

use std::collections::HashMap;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::app::surface::{run_admin_action, stable_scoped_command_id};

#[component]
pub(super) fn MailPollerControls(
    pollers: Vec<AdminMailPolling>,
    mut refresh: Signal<u64>,
) -> Element {
    let trigger_busy = use_signal(|| false);
    let pause_busy = use_signal(|| false);
    let mut trigger_command = use_signal(|| None::<Uuid>);
    let mut pause_commands = use_signal(HashMap::<String, Uuid>::new);
    let mut notice = use_signal(|| None::<String>);
    let mut error_code = use_signal(|| None::<String>);

    rsx! {
        Fragment {
            div { class: "admin-dashboard-actions",
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: trigger_busy(),
                    onclick: move |_| {
                        if trigger_busy() {
                            return;
                        }
                        notice.set(None);
                        error_code.set(None);
                        let command_id = trigger_command().unwrap_or_else(|| {
                            let command_id = Uuid::new_v4();
                            trigger_command.set(Some(command_id));
                            command_id
                        });
                        run_admin_action(
                            trigger_busy,
                            async move { trigger_mail_poll(command_id).await },
                            move |count| {
                                trigger_command.set(None);
                                notice.set(Some(t!(
                                    "dashboard-legacy-poll-triggered",
                                    count: count
                                )));
                            },
                            move |error| error_code.set(Some(error.code)),
                        );
                    },
                    {t!("dashboard-legacy-trigger-poll")}
                }
                for polling in pollers.iter().cloned() {
                    {
                        let desired_paused = !polling.paused;
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
                        let mailbox_key = polling.mailbox_key.clone();
                        rsx! {
                            button {
                                class: "admin-btn-secondary",
                                key: "{mailbox_key}",
                                r#type: "button",
                                disabled: pause_busy(),
                                onclick: move |_| {
                                    if pause_busy() {
                                        return;
                                    }
                                    notice.set(None);
                                    error_code.set(None);
                                    let polling = polling.clone();
                                    let signature = format!(
                                        "{}:{}:{}",
                                        polling.mailbox_key,
                                        polling.state_version,
                                        desired_paused
                                    );
                                    let command_id =
                                        stable_scoped_command_id(pause_commands, &signature);
                                    run_admin_action(
                                        pause_busy,
                                        async move {
                                            set_mail_paused(command_id, polling, desired_paused)
                                                .await
                                        },
                                        move |_| {
                                            pause_commands.write().remove(&signature);
                                            notice.set(Some(t!("dashboard-legacy-poll-updated")));
                                            refresh.set(refresh().saturating_add(1));
                                        },
                                        move |error| {
                                            if error.is_conflict() {
                                                refresh.set(refresh().saturating_add(1));
                                            }
                                            error_code.set(Some(error.code));
                                        },
                                    );
                                },
                                "{label}"
                            }
                        }
                    }
                }
            }
            if let Some(code) = error_code() {
                p { role: "alert", {super::super::admin_api_error_message(&code)} }
            }
            if let Some(message) = notice() {
                p { class: "admin-shell__inline-ok", role: "status", "{message}" }
            }
        }
    }
}
