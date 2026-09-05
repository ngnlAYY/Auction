//! Admin 改名历史页面。
//!
//! 页面沿用旧版的名称即时筛选、列表和页级确认对话框；回滚仍通过新版命令 API、当前用户
//! 版本和 CSRF 边界完成，浏览器不自行推导身份状态迁移。

use auction_web_shared::{
    AsyncState, AsyncStateKind, Dialog, Pager, Time, TruncatedText, legacy_t as t,
};
use dioxus::prelude::*;
use uuid::Uuid;

use std::cmp::Ordering;

use crate::api::{AdminRename, AdminRenamePage, fetch_admin_renames, revert_admin_rename};
use crate::app::surface::sort::{SortDirection, SortableTh, directed};
use crate::app::surface::stable_command_id;

use super::query::use_renames_query_state;
use super::{AdminActionFeedback, admin_api_error_message};

pub(super) fn page() -> Element {
    // 名称即时筛选与分页跟地址栏双向同步；回滚确认对话框和命令上下文是瞬态，不入 URL。
    let query = use_renames_query_state();
    let query_state = query.state;
    let mut refresh = use_signal(|| 0_u64);
    let mut busy_rename = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<AdminActionFeedback>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let mut selected = use_signal(|| None::<AdminRename>);
    let renames = use_resource(move || {
        let _ = refresh();
        let current = query_state();
        // 旧页面只按名称筛选，输入变化立即成为查询键。
        fetch_admin_renames("all".to_owned(), current.name.clone(), current.page)
    });
    // 名称即时筛选持一份 hook 克隆。
    let mut name_query = query.clone();

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
                // 错误态提供页内重试入口，不必刷新浏览器。
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
                on_select: move |rename| selected.set(Some(rename)),
                on_sort_change: move |(sort, direction)| {
                    // 排序只影响当前页展示顺序；筛选与翻页保持不变。
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    on_sort_change.push(next);
                },
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
            div { class: "admin-users-renames-filter",
                label {
                    r#for: "rename-name",
                    {t!("admin-renames-name-filter")}
                    input {
                        id: "rename-name",
                        r#type: "text",
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
                // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
                Dialog {
                    title: t!("admin-renames-revert-confirm-title"),
                    close_label: t!("admin-renames-cancel"),
                    busy: busy_rename().is_some(),
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

#[component]
fn RenameTable(
    page: AdminRenamePage,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    on_select: EventHandler<AdminRename>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    busy_rename: Signal<Option<String>>,
) -> Element {
    // 服务端不支持排序参数，浏览器只对当前页排序；未选择排序列时保持原始顺序。
    let mut renames = page.renames.clone();
    sort_renames(&mut renames, &sort, direction);
    rsx! {
        div { class: "admin-shell__table-wrap",
            table { class: "admin-shell__table", aria_busy: busy_rename().is_some().to_string(),
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
                        th { {t!("admin-renames-reverted")} }
                        th { {t!("admin-users-actions")} }
                    }
                }
                tbody {
                    for rename in renames {
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
