//! Admin 改名历史页面。
//! 页面沿用旧版的名称即时筛选、列表和页级确认对话框；回滚仍通过新版命令 API、当前用户
//! 版本和 CSRF 边界完成，浏览器不自行推导身份状态迁移。

use auction_web_shared::{AsyncState, AsyncStateKind, Dialog, Time, TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use std::{cell::RefCell, cmp::Ordering, rc::Rc};

use crate::api::{
    AdminRename, AdminRenamePage, AdminRenameRevertCommand, fetch_admin_renames,
    prepare_admin_rename_revert, revert_admin_rename,
};
use crate::app::surface::pending::{
    PendingCommandPanel, PendingCommands, set_if_alive, use_pending_command_gate,
    use_pending_commands_with_gate,
};
use crate::app::surface::sort::{SortDirection, SortableTh, directed};

use super::pagination::{AdminPager as Pager, use_admin_page, use_admin_pagination};
use super::query::use_renames_query_state;
use super::{AdminActionFeedback, admin_api_error_message};

pub(super) fn page() -> Element {
    // 名称即时筛选与分页跟地址栏双向同步；回滚确认对话框和命令上下文是瞬态，不入 URL。
    let query = use_renames_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.name.clone(), current.page)
    });
    let mut refresh = use_signal(|| 0_u64);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let command_gate = use_pending_command_gate();
    let pending = use_pending_commands_with_gate(refresh, Some(command_gate));
    let mut selected = use_signal(|| None::<AdminRename>);
    let pagination =
        use_admin_pagination("renames", &["page_size"], Some(200), i64::from(i32::MAX));
    let renames = use_admin_page(
        pagination.clone(),
        move || {
            let _ = refresh();
            requested_query()
        },
        |(name, page), size| fetch_admin_renames("all".to_owned(), name, page, size),
    );
    // 名称即时筛选持一份 hook 克隆。
    let mut name_query = query.clone();

    let confirm_revert = move |_| {
        let Some(rename) = selected() else {
            return;
        };
        if rename.reverted_at.is_some() || pending.any_busy() || pending.contains(&rename.user_id) {
            return;
        }
        // 首次准备成功后冻结 fence；重放只使用同一请求，不能重新读取用户版本。
        let frozen = Rc::new(RefCell::new(None::<AdminRenameRevertCommand>));
        let submitted = pending.submit(
            rename.user_id.clone(),
            t!("admin-renames-revert"),
            move |command_id| {
                let rename = rename.clone();
                let frozen = Rc::clone(&frozen);
                async move {
                    let existing = frozen.borrow().clone();
                    let command = match existing {
                        Some(command) => command,
                        None => {
                            let command = prepare_admin_rename_revert(command_id, &rename).await?;
                            *frozen.borrow_mut() = Some(command.clone());
                            command
                        }
                    };
                    revert_admin_rename(&command).await
                }
            },
            move |_| set_if_alive(notice, Some(AdminActionFeedback::Success)),
            move |code| {
                if matches!(code.as_str(), "stale_user_state" | "rename_not_revertable") {
                    let mut refresh = refresh;
                    refresh.set(refresh().saturating_add(1));
                }
                set_if_alive(notice, Some(AdminActionFeedback::Error(code)));
            },
        );
        if submitted {
            notice.set(None);
            selected.set(None);
        }
    };

    let state = renames.read_unchecked();
    let content = match &*state {
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
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| refresh.set(refresh().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query.clone();
            rsx! {
            RenameTable {
                page: page.clone(),
                sort: query_state().sort.clone(),
                direction: query_state().direction,
                on_select: move |rename: AdminRename| {
                    if !pending.any_busy() && !pending.contains(&rename.user_id) {
                        selected.set(Some(rename));
                    }
                },
                on_sort_change: move |(sort, direction)| {
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    on_sort_change.push(next);
                },
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
                pagination,
                page: page.page,
                page_size: page.page_size,
                total: Some(page.total),
                label: t!("page-renames-title"),
                on_page_change: move |next_page| {
                    let mut next = query_state();
                    next.page = next_page;
                    on_page_change.push(next);
                },
            }
            }
        }
    };

    rsx! {
        section { class: "admin-users-page", aria_labelledby: "admin-renames-title",
            h1 { id: "admin-renames-title", {t!("page-renames-title")} }
            PendingCommandPanel { owner: pending }
            div { class: "admin-users-renames-filter",
                label {
                    r#for: "rename-name",
                    {t!("admin-renames-name-filter")}
                    input {
                        id: "rename-name",
                        name: "name",
                        autocomplete: "off",
                        autocapitalize: "none",
                        spellcheck: "false",
                        r#type: "search",
                        maxlength: "256",
                        value: query_state().name,
                        oninput: move |event| {
                            // 即时筛选每次击键都同步地址栏；用 replace 避免击键淹没历史栈。
                            let mut next = query_state();
                            next.name = event.value();
                            next.page = 1;
                            name_query.replace(next);
                        },
                    }
                }
            }
            {content}
            if let Some(rename) = selected() {
                Dialog {
                    title: t!("admin-renames-revert-confirm-title"),
                    close_label: t!("admin-renames-cancel"),
                    busy: pending.any_busy(),
                    on_close: move |_| selected.set(None),
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
                            disabled: pending.any_busy(),
                            onclick: confirm_revert,
                            {t!("admin-users-action-confirm")}
                        }
                        button {
                            r#type: "button",
                            autofocus: true,
                            disabled: pending.any_busy(),
                            onclick: move |_| selected.set(None),
                            {t!("admin-renames-cancel")}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenameTable(
    page: AdminRenamePage,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_select: EventHandler<AdminRename>,
    on_sort_change: EventHandler<(String, SortDirection)>,
) -> Element {
    let pending = use_context::<PendingCommands>();
    let mut renames = page.renames;
    sort_renames(&mut renames, &sort, direction);
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table", aria_busy: pending.any_busy().to_string(),
                caption { class: "admin-visually-hidden", {t!("page-renames-title")} }
                auction_web_shared::TableColumns { layout: "admin-renames", widths: vec![21, 21, 9, 20, 20, 9] }
                thead {
                    tr {
                        SortableTh {
                            column: "previous",
                            label: t!("admin-renames-previous"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "next",
                            label: t!("admin-renames-next"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "actor",
                            label: t!("admin-renames-actor"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        SortableTh {
                            column: "created",
                            label: t!("admin-renames-created"),
                            active: sort.clone(),
                            direction,
                            on_sort: on_sort_change,
                        }
                        th { scope: "col", {t!("admin-renames-reverted")} }
                        th { scope: "col", {t!("admin-users-actions")} }
                    }
                }
                tbody {
                    for rename in renames {
                        RenameRow { key: "{rename.id}", rename, on_select }
                    }
                }
            }
        }
    }
}

#[component]
fn RenameRow(rename: AdminRename, on_select: EventHandler<AdminRename>) -> Element {
    let pending = use_context::<PendingCommands>();
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
                        disabled: pending.any_busy() || pending.contains(&rename_for_action.user_id),
                        onclick: move |_| on_select.call(rename_for_action.clone()),
                        {t!("admin-renames-revert")}
                    }
                }
            }
        }
    }
}

/// 对当前页改名记录排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_renames(renames: &mut [AdminRename], column: &str, direction: SortDirection) {
    if column.is_empty() {
        return;
    }
    renames.sort_by(|left, right| {
        directed(
            match column {
                "created" => left.created_at.cmp(&right.created_at),
                "previous" => left.previous_username.cmp(&right.previous_username),
                "next" => left.next_username.cmp(&right.next_username),
                "actor" => left.actor.cmp(&right.actor),
                _ => Ordering::Equal,
            },
            direction,
        )
    });
}
