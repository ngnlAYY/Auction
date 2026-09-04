//! Admin 改名历史页面。
//!
//! 页面沿用旧版的名称即时筛选、列表和页级确认对话框；回滚仍通过新版命令 API、当前用户
//! 版本和 CSRF 边界完成，浏览器不自行推导身份状态迁移。

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, Time, TruncatedText, legacy_t as t};
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use crate::api::{AdminRename, AdminRenamePage, fetch_admin_renames, revert_admin_rename};

use super::{AdminActionFeedback, admin_api_error_message};

pub(super) fn page() -> Element {
    let mut name = use_signal(String::new);
    let mut page_number = use_signal(|| 1_i64);
    let mut refresh = use_signal(|| 0_u64);
    let mut busy_rename = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let mut selected = use_signal(|| None::<AdminRename>);
    let renames = use_resource(move || {
        let _ = refresh();
        // 旧页面只按名称筛选，输入变化立即成为查询键。
        fetch_admin_renames("all".to_owned(), name(), page_number())
    });

    let confirm_revert = move |_| {
        let Some(rename) = selected() else {
            return;
        };
        if rename.reverted_at.is_some() || busy_rename().is_some() {
            return;
        }
        notice.set(None);
        busy_rename.set(Some(rename.id.clone()));
        let command_id = stable_command_id(command_state, format!("revert:{}", rename.id));
        let mut busy_rename = busy_rename;
        spawn(async move {
            match revert_admin_rename(command_id, &rename).await {
                Ok(_) => {
                    notice.set(Some(AdminActionFeedback::Success));
                    command_state.set(None);
                    selected.set(None);
                    refresh.set(refresh().saturating_add(1));
                }
                Err(code) => notice.set(Some(AdminActionFeedback::Error(code))),
            }
            busy_rename.set(None);
        });
    };

    let state = renames.read_unchecked().clone();
    let content = match state {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-renames-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(&error),
                detail: "",
            }
        },
        Some(Ok(page)) => rsx! {
            RenameTable {
                page: page.clone(),
                on_select: move |rename| selected.set(Some(rename)),
                busy_rename,
            }
            if page.renames.is_empty() {
                AsyncState {
                    kind: AsyncStateKind::Empty,
                    title: t!("admin-renames-empty"),
                    detail: "",
                }
            }
            if let Some(feedback) = notice() {
                p {
                    class: if feedback.is_success() {
                        "admin-shell__inline-ok"
                    } else {
                        "admin-shell__inline-error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("admin-renames-revert-success"))}
                }
            }
            Pager {
                page: page.page,
                page_size: page.page_size,
                total: Some(page.total),
                label: t!("page-renames-title"),
                previous_label: t!("admin-pagination-previous"),
                status: move |(page, pages): (i64, i64)| {
                    t!("admin-pagination-status", page: page, pages: pages)
                },
                next_label: t!("admin-pagination-next"),
                jump_label: t!("admin-pagination-input"),
                jump_button_label: t!("admin-pagination-go"),
                on_page_change: move |next_page| page_number.set(next_page),
            }
        },
    };

    rsx! {
        section { class: "admin-users-page", aria_labelledby: "admin-renames-title",
            h1 { id: "admin-renames-title", {t!("page-renames-title")} }
            div { class: "admin-users-renames-filter",
                label {
                    r#for: "rename-name",
                    {t!("admin-renames-name-filter")}
                    input {
                        id: "rename-name",
                        r#type: "text",
                        maxlength: "256",
                        value: name(),
                        oninput: move |event| {
                            name.set(event.value());
                            page_number.set(1);
                        },
                    }
                }
            }
            {content}
            if let Some(rename) = selected() {
                div { class: "admin-shell__dialog-backdrop", role: "presentation",
                    section {
                        class: "admin-shell__dialog",
                        role: "dialog",
                        aria_modal: "true",
                        aria_labelledby: "rename-dialog-title",
                        onkeydown: move |event| {
                            if event.key() == Key::Escape && busy_rename().is_none() {
                                event.stop_propagation();
                                selected.set(None);
                            }
                        },
                        h2 { id: "rename-dialog-title", {t!("admin-renames-revert-confirm-title")} }
                        p {
                            {t!(
                                "admin-renames-revert-confirm-description",
                                from: rename.next_username,
                                to: rename.previous_username
                            )}
                        }
                        div { class: "admin-shell__dialog-actions",
                            button {
                                r#type: "button",
                                disabled: busy_rename().is_some(),
                                onclick: confirm_revert,
                                {t!("admin-users-action-confirm")}
                            }
                            button {
                                r#type: "button",
                                autofocus: true,
                                disabled: busy_rename().is_some(),
                                onclick: move |_| selected.set(None),
                                {t!("admin-renames-cancel")}
                            }
                        }
                    }
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

#[component]
fn RenameTable(
    page: AdminRenamePage,
    on_select: EventHandler<AdminRename>,
    busy_rename: Signal<Option<String>>,
) -> Element {
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table", aria_busy: busy_rename().is_some().to_string(),
                thead {
                    tr {
                        th { {t!("admin-renames-previous")} }
                        th { {t!("admin-renames-next")} }
                        th { {t!("admin-renames-actor")} }
                        th { {t!("admin-renames-created")} }
                        th { {t!("admin-renames-reverted")} }
                        th { {t!("admin-users-actions")} }
                    }
                }
                tbody {
                    for rename in page.renames {
                        RenameRow { rename, on_select }
                    }
                }
            }
        }
    }
}

#[component]
fn RenameRow(rename: AdminRename, on_select: EventHandler<AdminRename>) -> Element {
    let is_reverted = rename.reverted_at.is_some();
    let rename_for_action = rename.clone();
    rsx! {
        tr {
            td {
                TruncatedText { value: rename.previous_username }
            }
            td {
                TruncatedText { value: rename.next_username }
            }
            td {
                TruncatedText { value: rename.actor }
            }
            td {
                Time { value: rename.created_at.clone() }
            }
            td {
                if let Some(reverted_at) = rename.reverted_at {
                    Time { value: reverted_at }
                }
            }
            td {
                if is_reverted {
                    {t!("admin-renames-no-action")}
                } else {
                    button {
                        r#type: "button",
                        onclick: move |_| on_select.call(rename_for_action.clone()),
                        {t!("admin-renames-revert")}
                    }
                }
            }
        }
    }
}
