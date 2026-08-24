//! Admin 改名历史页面。
//!
//! 页面只显示安全历史投影；回滚操作通过后端 Users 命令、当前用户版本和 CSRF 边界完成，
//! 浏览器不自行推导身份状态迁移。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminRename, AdminRenamePage, fetch_admin_renames, revert_admin_rename};

use super::pagination::AdminPager;
use super::{AdminLoadError, AdminPanel, format_admin_time};

pub(super) fn page() -> Element {
    let mut user_id = use_signal(|| "all".to_owned());
    let mut name = use_signal(String::new);
    let mut active_user_id = use_signal(|| "all".to_owned());
    let mut active_name = use_signal(String::new);
    let mut page_number = use_signal(|| 1_i64);
    let mut refresh = use_signal(|| 0_u64);
    let mut busy_rename = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<bool>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let renames = use_resource(move || {
        let _ = refresh();
        fetch_admin_renames(active_user_id(), active_name(), page_number())
    });

    let load = move |_| {
        let next_user_id = match user_id().trim() {
            "" => "all".to_owned(),
            value => value.to_owned(),
        };
        let next_name = name().trim().to_owned();
        active_user_id.set(next_user_id);
        active_name.set(next_name);
        page_number.set(1);
        refresh.set(refresh().saturating_add(1));
    };

    let on_revert = move |rename: AdminRename| {
        if rename.reverted_at.is_some() || busy_rename().is_some() {
            return;
        }
        notice.set(None);
        busy_rename.set(Some(rename.id.clone()));
        let command_id = stable_command_id(
            command_state,
            format!("revert:{}:{}", rename.id, rename.user_state_version),
        );
        let mut busy_rename = busy_rename;
        spawn(async move {
            let successful = revert_admin_rename(command_id, &rename).await.is_ok();
            notice.set(Some(successful));
            if successful {
                command_state.set(None);
                refresh.set(refresh().saturating_add(1));
            }
            busy_rename.set(None);
        });
    };

    match &*renames.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-renames-title"), detail: t!("admin-renames-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-renames-title"),
                detail: t!("admin-renames-error"),
                refresh,
            }
        },
        Some(Ok(page)) => rsx! {
            section { class: "admin-panel", aria_labelledby: "admin-renames-title",
                h1 { id: "admin-renames-title", {t!("page-renames-title")} }
                p { class: "admin-panel__note", {t!("page-renames-note")} }
                div { class: "admin-panel__actions",
                    label {
                        {t!("admin-renames-user-filter")}
                        input {
                            maxlength: "64",
                            value: user_id(),
                            oninput: move |event| user_id.set(event.value()),
                        }
                    }
                    label {
                        {t!("admin-renames-name-filter")}
                        input {
                            maxlength: "256",
                            value: name(),
                            oninput: move |event| name.set(event.value()),
                        }
                    }
                    button { r#type: "button", onclick: load, {t!("admin-renames-load")} }
                }
                if let Some(successful) = notice() {
                    p {
                        class: if successful {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if successful { "status" } else { "alert" },
                        if successful {
                            {t!("admin-renames-revert-success")}
                        } else {
                            {t!("admin-renames-revert-error")}
                        }
                    }
                }
                RenameTable { page: page.clone(), page_number, on_revert, busy_rename }
            }
        },
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

#[component]
fn RenameTable(
    page: AdminRenamePage,
    page_number: Signal<i64>,
    on_revert: EventHandler<AdminRename>,
    busy_rename: Signal<Option<String>>,
) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            aria_labelledby: "admin-renames-table-title",
            aria_busy: busy_rename().is_some().to_string(),
            h2 { id: "admin-renames-table-title", {t!("admin-renames-total", total: page.total)} }
            if page.renames.is_empty() {
                p { class: "admin-panel__note", role: "status", {t!("admin-renames-empty")} }
            } else {
                div { class: "public-auction-catalog admin-renames__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-renames-id")} }
                                th { {t!("admin-renames-user")} }
                                th { {t!("admin-renames-previous")} }
                                th { {t!("admin-renames-next")} }
                                th { {t!("admin-renames-actor")} }
                                th { {t!("admin-renames-created")} }
                                th { {t!("admin-renames-reverted")} }
                                th { {t!("catalog-actions")} }
                            }
                        }
                        tbody {
                            for rename in page.renames {
                                RenameRow { rename, on_revert, busy_rename }
                            }
                        }
                    }
                }
            }
            AdminPager {
                page: page.page,
                page_size: page.page_size,
                total: page.total,
                label: t!("admin-renames-pagination"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        }
    }
}

#[component]
fn RenameRow(
    rename: AdminRename,
    on_revert: EventHandler<AdminRename>,
    busy_rename: Signal<Option<String>>,
) -> Element {
    let is_reverted = rename.reverted_at.is_some();
    let is_busy = busy_rename().as_deref() == Some(rename.id.as_str());
    let mut confirmation = use_signal(|| false);
    let rename_for_action = rename.clone();
    let previous_username = rename.previous_username.clone();
    let next_username = rename.next_username.clone();
    rsx! {
        tr { class: "admin-renames__row",
            td { "data-label": t!("admin-renames-id"), TruncatedText { value: rename.id } }
            td { "data-label": t!("admin-renames-user"), TruncatedText { value: rename.user_id } }
            td { "data-label": t!("admin-renames-previous"), TruncatedText { value: previous_username.clone() } }
            td { "data-label": t!("admin-renames-next"), TruncatedText { value: next_username.clone() } }
            td { "data-label": t!("admin-renames-actor"), TruncatedText { value: rename.actor } }
            td { "data-label": t!("admin-renames-created"),
                time { datetime: rename.created_at.clone(), "{format_admin_time(&rename.created_at)}" }
            }
            td { "data-label": t!("admin-renames-reverted"),
                if let Some(reverted_at) = rename.reverted_at.clone() {
                    time { datetime: reverted_at.clone(), "{format_admin_time(&reverted_at)}" }
                } else {
                    span { class: "admin-renames__status admin-renames__status--pending", {t!("value-empty")} }
                }
            }
            td { "data-label": t!("catalog-actions"), class: "admin-renames__actions",
                if is_reverted {
                    span { class: "admin-renames__status admin-renames__status--reverted", {t!("admin-renames-no-action")} }
                } else {
                    button {
                        class: "admin-btn-danger",
                        r#type: "button",
                        disabled: busy_rename().is_some(),
                        onclick: move |_| confirmation.set(true),
                        if is_busy { {t!("admin-action-working")} } else { {t!("admin-renames-revert")} }
                    }
                }
            }
        }
        if confirmation() {
            tr { class: "admin-renames__dialog-row",
                td { colspan: "8",
                    div { class: "admin-renames__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-renames__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-renames-confirm-title",
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy_rename().is_some() {
                                    confirmation.set(false);
                                }
                            },
                            h2 { id: "admin-renames-confirm-title", {t!("admin-renames-revert-confirm-title")} }
                            p {
                                {t!(
                                    "admin-renames-revert-confirm-description",
                                    from: next_username.clone(),
                                    to: previous_username.clone()
                                )}
                            }
                            div { class: "admin-panel__actions",
                                button {
                                    r#type: "button",
                                    autofocus: true,
                                    disabled: busy_rename().is_some(),
                                    onclick: move |_| confirmation.set(false),
                                    {t!("admin-renames-cancel")}
                                }
                                button {
                                    class: "admin-btn-danger",
                                    r#type: "button",
                                    disabled: busy_rename().is_some(),
                                    onclick: move |_| {
                                        confirmation.set(false);
                                        on_revert.call(rename_for_action.clone());
                                    },
                                    {t!("admin-renames-revert")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
