//! Admin Dashboard 邮件轮询控制。
//! 命令由稳定 Dashboard 页面持有，后台快照失败卸载控件时仍保留原参数与 UUID；
//! 暂停冲突会刷新唯一 Dashboard 快照，但不会把 stale 结果伪装成成功。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminMailPolling, set_mail_paused, trigger_mail_poll};
use crate::surface::pending::{PendingCommands, set_if_alive};

#[component]
pub(super) fn MailPollerControls(pollers: Vec<AdminMailPolling>, refresh: Signal<u64>) -> Element {
    let owner = use_context::<PendingCommands>();
    let mut notice = use_signal(|| None::<String>);
    let mut error_code = use_signal(|| None::<String>);

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
                        owner.submit(
                            "mail-poll-trigger".to_owned(),
                            t!("dashboard-legacy-trigger-poll"),
                            |command_id| async move {
                                trigger_mail_poll(command_id).await.map_err(|error| error.code)
                            },
                            move |count| {
                                set_if_alive(notice, Some(t!(
                                    "dashboard-legacy-poll-triggered",
                                    count: count
                                )));
                            },
                            move |code| set_if_alive(error_code, Some(code)),
                        );
                    },
                    {t!("dashboard-legacy-trigger-poll")}
                }
                for polling in pollers {
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
                        let command_label = label.clone();
                        rsx! {
                            button {
                                class: "admin-btn-secondary",
                                key: "{mailbox_key}",
                                r#type: "button",
                                disabled: owner.contains("mail-polling"),
                                onclick: move |_| {
                                    if owner.contains("mail-polling") {
                                        return;
                                    }
                                    notice.set(None);
                                    error_code.set(None);
                                    let polling = polling.clone();
                                    // 保留本页暂停/恢复串行策略；未知结果必须先原样解决。
                                    owner.submit(
                                        "mail-polling".to_owned(),
                                        command_label.clone(),
                                        move |command_id| {
                                            let polling = polling.clone();
                                            async move {
                                                let result = set_mail_paused(command_id, polling, desired_paused).await;
                                                if result.as_ref().is_err_and(crate::api::AdminApiFailure::is_conflict) {
                                                    let mut refresh = refresh;
                                                    refresh.set(refresh().saturating_add(1));
                                                }
                                                result.map_err(|error| error.code)
                                            }
                                        },
                                        move |_| set_if_alive(notice, Some(t!("dashboard-legacy-poll-updated"))),
                                        move |code| set_if_alive(error_code, Some(code)),
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
