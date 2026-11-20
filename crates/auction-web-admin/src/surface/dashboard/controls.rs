//! Admin Dashboard 邮件轮询控制。
//! 命令由认证会话持有，后台快照失败卸载控件时仍保留原参数与 UUID；
//! 暂停冲突会刷新唯一 Dashboard 快照，但不会把 stale 结果伪装成成功。

#[cfg(test)]
use std::rc::Rc;

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::api::{AdminMailPolling, AdminWriteFailure, set_mail_paused, trigger_mail_poll};
#[cfg(test)]
use crate::surface::auctions::draft_page_tests::RecordRead;
use crate::surface::mail::{MAILBOX_ENTITY_PREFIX, mailbox_command_entity};
use crate::surface::pending::{CommandDelivery, CommandReceipt, PendingCommands, set_if_alive};

#[component]
pub(super) fn MailPollerControls(
    pollers: Vec<AdminMailPolling>,
    refresh: Signal<u64>,
    loaded_generation: u64,
) -> Element {
    let owner = use_context::<PendingCommands>();
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
                        error_code.set(None);
                        owner.submit_with_receipt(
                            "mail-poll-trigger".to_owned(),
                            t!("dashboard-legacy-trigger-poll"),
                            |command_id| async move {
                                trigger_mail_poll(command_id).await
                            },
                            move |count, source_alive| {
                                if source_alive {
                                    set_if_alive(error_code, None);
                                }
                                CommandDelivery::Receipt(Some(CommandReceipt::Text(t!("dashboard-legacy-poll-triggered", count: count))))
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
                                        #[cfg(test)]
                                        if let Some(reply) = try_consume_context::<Rc<RecordRead<AdminMailPolling>>>() {
                                            return reply.fetch().await.map_err(crate::api::AdminWriteFailure::outcome_unknown);
                                        }
                                        let result = set_mail_paused(command_id, polling, desired_paused).await;
                                        observe_pause_resume(result, refresh)
                                    }
                                },
                                move |_, source_alive| {
                                    if source_alive {
                                        set_if_alive(error_code, None);
                                    }
                                    // 聚合读取失败会卸载轮询控件，已接受的回执由会话交付。
                                    CommandDelivery::Receipt(Some(CommandReceipt::Text(t!("dashboard-legacy-poll-updated"))))
                                },
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
        }
    }
}

fn observe_pause_resume(
    result: Result<AdminMailPolling, AdminWriteFailure>,
    mut refresh: Signal<u64>,
) -> Result<AdminMailPolling, AdminWriteFailure> {
    if result.as_ref().is_err_and(|failure| failure.observed_response_status() == Some(409))
        && let Ok(mut generation) = refresh.try_write()
    {
        *generation = generation.saturating_add(1);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    use dioxus::prelude::{
        dioxus_core, dioxus_elements, dioxus_signals, rsx, use_resource, use_signal,
    };

    use super::observe_pause_resume;
    use crate::api::{AdminWriteEndpoint, AdminWriteFailure};
    use crate::surface::auctions::draft_page_tests::{button, click_button, mount_writer};
    use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

    #[test]
    fn dashboard_pause_resume_keeps_pending_when_the_retry_was_not_sent() {
        assert_no_refresh_on_retry(AdminWriteFailure::not_sent("http_409"));
    }

    #[test]
    fn dashboard_pause_resume_keeps_pending_when_the_retry_has_no_response_status() {
        assert_no_refresh_on_retry(AdminWriteFailure::outcome_unknown("network_error"));
    }

    fn assert_no_refresh_on_retry(retry_failure: AdminWriteFailure) {
        let outcomes = Rc::new(RefCell::new(VecDeque::from([
            AdminWriteFailure::from_test_http(AdminWriteEndpoint::General, 503, b"{"),
            retry_failure,
        ])));
        let reads = Rc::new(Cell::new(0));
        let (mut page, _) = mount_writer({
            let outcomes = outcomes.clone();
            let reads = reads.clone();
            move || {
                let refresh = use_signal(|| 0_u64);
                let reads = reads.clone();
                let _snapshot = use_resource(move || {
                    let _ = refresh();
                    reads.set(reads.get() + 1);
                    async {}
                });
                let pending = use_pending_commands(refresh, |key| key == "mailbox:main");
                let outcomes = outcomes.clone();
                let label = "提交轮询控制";
                rsx! {
                    PendingCommandPanel { owner: pending }
                    button {
                        onclick: move |_| {
                            let outcomes = outcomes.clone();
                            pending.submit("mailbox:main".into(), "轮询控制".into(), move |_| {
                                let failure = outcomes.borrow_mut().pop_front().unwrap();
                                async move { observe_pause_resume(Err(failure), refresh) }
                            }, |_| panic!("没有写响应不能交付成功回执"), |_| ());
                        },
                        "{label}"
                    }
                }
            }
        });
        assert_eq!(reads.get(), 1);
        click_button(&mut page, "提交轮询控制");
        assert!(button(&page, "原样重试").is_some());
        click_button(&mut page, "原样重试");
        assert!(outcomes.borrow().is_empty());
        assert_eq!(reads.get(), 1, "缺少写响应状态不得重读 Dashboard");
        assert!(button(&page, "原样重试").is_some());
        assert!(button(&page, "关闭已拒绝操作").is_none());
    }
}
