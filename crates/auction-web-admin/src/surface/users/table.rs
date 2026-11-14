//! Admin 用户筛选、列表和分页。

use std::cmp::Ordering;

use auction_web_shared::t;
use dioxus::prelude::*;

use crate::surface::pagination::{AdminPager as Pager, AdminPagination};
use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminUser, AdminUserStatusFilter};
use crate::surface::AdminActionFeedback;
use crate::surface::sort::{SortDirection, SortableTh, sort_rows};

use super::create::AdminUserGeneratedKey;
use super::dialogs::{parse_status_filter, status_filter_value};
use super::row::AdminUserRow;

#[component]
pub(super) fn AdminUserTable(
    page: crate::api::AdminUserPage,
    pagination: AdminPagination,
    mut name_draft: Signal<String>,
    /// 当前状态筛选（地址栏恢复后的值）；变更经回调由页面写回 URL。
    status: AdminUserStatusFilter,
    /// 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `on_sort_change` 由页面写回 URL。
    sort: String,
    direction: SortDirection,
    /// 提交名称草稿：由页面 push 到地址栏并回到第一页。
    on_name_apply: EventHandler<String>,
    on_status_change: EventHandler<AdminUserStatusFilter>,
    on_page_change: EventHandler<i64>,
    on_sort_change: EventHandler<(String, SortDirection)>,
    reload_generation: Signal<u64>,
    action_busy: Signal<bool>,
    action_feedback: Signal<Option<AdminActionFeedback>>,
    create_notice: Signal<Option<String>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    let is_empty = page.users.is_empty();
    let mut users = page.users;
    sort_users(&mut users, &sort, direction);
    rsx! {
        form {
            class: "admin-users-filter",
            autocomplete: "off",
            onsubmit: move |event| {
                event.prevent_default();
                on_name_apply.call(name_draft().trim().to_owned());
            },
            label {
                r#for: "user-name-filter",
                {t!("admin-users-filter-name")}
                input {
                    id: "user-name-filter",
                    name: "name",
                    r#type: "search",
                    autocapitalize: "none",
                    spellcheck: "false",
                    enterkeyhint: "search",
                    maxlength: "256",
                    value: name_draft(),
                    oninput: move |event| {
                        name_draft.set(event.value());
                    },
                }
            }
            label {
                r#for: "user-status-filter",
                {t!("admin-users-filter-status")}
                select {
                    id: "user-status-filter",
                    name: "status",
                    value: status_filter_value(status),
                    onchange: move |event| {
                        on_status_change.call(parse_status_filter(&event.value()));
                    },
                    option { value: "all", {t!("admin-users-status-all")} }
                    option { value: "active", {t!("admin-users-status-active")} }
                    option { value: "blacklisted", {t!("admin-users-status-blacklisted")} }
                }
            }
            button { r#type: "submit", class: "admin-btn-secondary", {t!("admin-users-filter-apply")} }
        }
        div { class: "admin-users-table-wrap",
            table { class: "admin-users-table",
                aria_label: t!("page-users-title"),
                // 列序：名称、论坛 ID、状态、封禁理由、次数、注册时间、操作；为四个操作按钮预留同排空间。
                auction_web_shared::TableColumns { layout: "admin-users", widths: vec![12, 10, 6, 14, 5, 13, 40] }
                thead { tr {
                    SortableTh {
                        column: "username",
                        label: t!("admin-users-username"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-forum-account-id")} }
                    SortableTh {
                        column: "status",
                        label: t!("admin-users-status"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-blacklist-history")} }
                    SortableTh {
                        column: "blacklist_count",
                        label: t!("admin-users-blacklist-count"),
                        active: sort.clone(),
                        direction,
                        on_sort: on_sort_change,
                    }
                    SortableTh {
                        column: "created_at",
                        label: t!("admin-users-created-at"),
                        active: sort,
                        direction,
                        on_sort: on_sort_change,
                    }
                    th { scope: "col", {t!("admin-users-actions")} }
                } }
                tbody {
                    for user in users {
                        AdminUserRow {
                            key: "{user.id}",
                            user,
                            reload_generation,
                            busy: action_busy,
                            feedback: action_feedback,
                            on_generated_key,
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-users-no-results"),
                detail: "",
            }
        }
        if let Some(feedback) = action_feedback() {
            p {
                class: if feedback.is_success() {
                    "admin-shell__inline-ok"
                } else {
                    "admin-shell__inline-error"
                },
                role: if feedback.is_success() { "status" } else { "alert" },
                {feedback.message(t!("admin-users-updated"))}
            }
        }
        if let Some(notice) = create_notice() {
            p { class: "admin-shell__inline-ok", role: "status", {notice} }
        }
        Pager {
            pagination,
            page: page.pagination.page,
            page_size: page.pagination.page_size,
            total: Some(page.pagination.total),
            label: t!("page-users-title"),
            on_page_change,
        }
    }
}

/// 对当前页用户行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
fn sort_users(users: &mut [AdminUser], column: &str, direction: SortDirection) {
    sort_rows(users, column, direction, |left, right, column| match column {
        "username" => left.username.cmp(&right.username),
        "status" => left.status.cmp(&right.status),
        "blacklist_count" => left.blacklist_count.cmp(&right.blacklist_count),
        "created_at" => left.created_at.cmp(&right.created_at),
        _ => Ordering::Equal,
    });
}
