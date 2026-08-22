//! Admin 邮件轮询状态与入站审查页面。
//!
//! 页面只展示后端安全投影；暂停动作携带服务端返回的轮询版本，冲突时重新读取权威状态。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    AdminMailIntake, AdminMailIntakePage, AdminMailPolling, fetch_mail_intakes, fetch_mail_state,
    set_mail_paused,
};

use super::AdminPanel;

/// 加载并展示指定邮箱的轮询状态和入站邮件摘要。
pub(super) fn page() -> Element {
    let mut mailbox_key = use_signal(|| "main".to_owned());
    let mut active_mailbox = use_signal(|| "main".to_owned());
    let mut refresh = use_signal(|| 0_u64);
    let polling = use_resource(move || {
        let _ = refresh();
        fetch_mail_state(active_mailbox())
    });
    let intakes = use_resource(move || {
        let _ = refresh();
        fetch_mail_intakes(active_mailbox(), 1)
    });
    let mut notice = use_signal(|| None::<bool>);

    let load_mailbox = move |_| {
        let value = mailbox_key().trim().to_owned();
        if !value.is_empty() {
            notice.set(None);
            active_mailbox.set(value);
            refresh.set(refresh().saturating_add(1));
        } else {
            notice.set(Some(false));
        }
    };

    match (&*polling.read_unchecked(), &*intakes.read_unchecked()) {
        (None, _) | (_, None) => rsx! {
            AdminPanel { title: t!("page-mails-title"), detail: t!("admin-mail-loading") }
        },
        (Some(Err(_)), _) | (_, Some(Err(_))) => rsx! {
            AdminPanel { title: t!("page-mails-title"), detail: t!("admin-mail-error") }
        },
        (Some(Ok(polling)), Some(Ok(intakes))) => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-mail-title",
                h1 { id: "admin-mail-title", {t!("page-mails-title")} }
                div { class: "admin-panel__actions",
                    label {
                        {t!("admin-mail-mailbox")}
                        input {
                            maxlength: "256",
                            value: mailbox_key(),
                            oninput: move |event| mailbox_key.set(event.value()),
                        }
                    }
                    button { r#type: "button", onclick: load_mailbox, {t!("admin-mail-load")} }
                }
                MailPollingCard { polling: polling.clone(), notice, refresh }
                MailIntakeTable { page: intakes.clone() }
                if let Some(successful) = notice() {
                    p { class: "admin-panel__note", role: "status",
                        if successful { {t!("admin-mail-action-success")} }
                        else { {t!("admin-mail-action-error")} }
                    }
                }
            }
        },
    }
}

#[component]
fn MailPollingCard(
    polling: AdminMailPolling,
    mut notice: Signal<Option<bool>>,
    mut refresh: Signal<u64>,
) -> Element {
    let desired_paused = !polling.paused;
    let label = if polling.paused { t!("admin-mail-resume") } else { t!("admin-mail-pause") };
    let state_label =
        if polling.paused { t!("admin-mail-paused") } else { t!("admin-mail-running") };
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-mail-polling-title",
            h2 { id: "admin-mail-polling-title", {t!("admin-mail-polling-title")} }
            p { class: "admin-panel__note",
                {t!(
                    "admin-mail-polling-summary",
                    mailbox: polling.mailbox_key.clone(),
                    status: state_label,
                    version: polling.state_version,
                    failures: polling.consecutive_failures
                )}
            }
            p { class: "admin-panel__note",
                {t!(
                    "admin-mail-polling-times",
                    last: polling.last_poll_at.clone(),
                    next: polling.next_poll_at.clone()
                )}
            }
            button {
                r#type: "button",
                onclick: move |_| {
                    let polling = polling.clone();
                    let mut notice = notice;
                    let mut refresh = refresh;
                    spawn(async move {
                        match set_mail_paused(polling, desired_paused).await {
                            Ok(_) => {
                                notice.set(Some(true));
                                refresh.set(refresh().saturating_add(1));
                            }
                            Err(_) => notice.set(Some(false)),
                        }
                    });
                },
                "{label}"
            }
        }
    }
}

#[component]
fn MailIntakeTable(page: AdminMailIntakePage) -> Element {
    rsx! {
        section { class: "admin-panel", aria_labelledby: "admin-mail-intake-title",
            h2 { id: "admin-mail-intake-title", {t!("admin-mail-intake-title")} }
            p { class: "admin-panel__note", {t!("admin-mail-intake-total", total: page.pagination.total)} }
            div { class: "public-auction-catalog",
                table {
                    thead {
                        tr {
                            th { {t!("admin-mail-intake-id")} }
                            th { {t!("admin-mail-intake-world")} }
                            th { {t!("admin-mail-intake-kind")} }
                            th { {t!("admin-mail-intake-status")} }
                            th { {t!("admin-mail-intake-decision")} }
                            th { {t!("admin-mail-intake-received")} }
                        }
                    }
                    tbody {
                        for intake in page.intakes {
                            MailIntakeRow { intake }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MailIntakeRow(intake: AdminMailIntake) -> Element {
    rsx! {
        tr {
            td { "{intake.id}" }
            td { "{intake.world_id}" }
            td { "{intake.kind}" }
            td { "{intake.status}" }
            td { "{intake.decision_code}" }
            td { "{intake.received_at}" }
        }
    }
}
