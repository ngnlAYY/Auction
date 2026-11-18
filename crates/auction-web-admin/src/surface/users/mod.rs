//! Admin 用户管理页面。
//! 页面只维护筛选、表单和安全用户投影；页级命令宿主冻结完整负载和 command_id，
//! 原样重试并使用返回的 state_version，一次性 Key 由稳定页面接收。

#[cfg(test)]
use std::{cell::Cell, rc::Rc};

use auction_web_shared::t;
use auction_web_shared::{AsyncState, AsyncStateKind, use_popstate_revision};
use dioxus::prelude::*;

use super::pagination::{use_admin_page, use_admin_pagination};
use super::pending::{
    PendingCommandPanel, use_pending_command_gate, use_pending_commands_with_gate,
};
use super::query::use_users_query_state;
use super::{AdminActionFeedback, admin_api_error_message};
use crate::api::{AdminUserStatusFilter, fetch_admin_users};

mod create;
mod dialogs;
pub(super) mod fields;
mod row;
mod table;

use create::{AdminUserCreateForm, AdminUserGeneratedKeyDialog, UserKeyDelivery};
use table::{AdminUserFilters, AdminUserTable};

#[cfg(test)]
pub(super) struct UsersPageFixture {
    pub fetch: Box<dyn Fn(String, i64, i64) -> crate::api::AdminUserPage>,
    pub retry_correction: Cell<Option<EventHandler<()>>>,
}

/// 加载并展示 Admin 用户管理页面。
pub(super) fn page() -> Element {
    // 已应用的名称筛选、状态筛选与分页跟地址栏双向同步；名称输入框草稿只在提交时入 URL。
    let query = use_users_query_state();
    let query_state = query.state;
    let history_revision = use_signal(|| 0_u64);
    use_popstate_revision(history_revision);
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.name.clone(), current.status, current.page)
    });
    // 主动排序与翻页保留草稿；历史导航用独立版本恢复地址里的文本，即使筛选内容相同。
    let applied_filter = use_memo(move || {
        let current = query_state.read();
        (current.name.clone(), current.status)
    });
    let mut reload_generation = use_signal(|| 0_u64);
    let mut create_open = use_signal(|| false);
    let mut action_busy = use_signal(|| false);
    let command_gate = use_pending_command_gate();
    let delivery = UserKeyDelivery { gate: command_gate, generated: use_signal(|| None) };
    let pending = use_pending_commands_with_gate(reload_generation, Some(command_gate));
    // effect 仅用于既有控件的忙态展示；首次提交/重试在执行边界同步占用同一门禁。
    use_effect(move || action_busy.set(pending.any_busy() || command_gate.blocked()));
    let mut action_feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut create_notice = use_signal(|| None::<String>);
    let pagination = use_admin_pagination("users", &["page_size"], Some(200), i64::from(i32::MAX));
    let users = use_admin_page(
        pagination.clone(),
        move || {
            let _ = reload_generation();
            requested_query()
        },
        |(name, status, page), size| async move {
            #[cfg(test)]
            if let Some(fixture) = try_consume_context::<Rc<UsersPageFixture>>() {
                return Ok((fixture.fetch)(name, page, size));
            }
            fetch_admin_users(name, status, page, size).await
        },
    );

    let mut correction_failed = use_signal(|| false);
    let mut correct_page = move || {
        let failed = match users.read().as_ref() {
            Some(Ok(page)) => !query_state().correct_page(query_state, &page.pagination),
            _ => false,
        };
        correction_failed.set(failed);
    };
    use_effect(correct_page);
    let retry_correction = use_callback(move |()| correct_page());
    // 用户页先按权威页数纠偏，避免旧页交给仍把纠偏当作翻页的 AdminPager 包装。
    let result = users.read_unchecked();
    let needs_correction = result.as_ref().is_some_and(|result| {
        result
            .as_ref()
            .is_ok_and(|page| query_state.read().page_correction(&page.pagination).is_some())
    });
    let state = result.as_ref().filter(|_| !needs_correction).cloned();
    let content = match state {
        None if needs_correction && correction_failed() => {
            #[cfg(test)]
            if let Some(fixture) = try_consume_context::<Rc<UsersPageFixture>>() {
                fixture.retry_correction.set(Some(retry_correction));
            }
            rsx! {
                AsyncState {
                    kind: AsyncStateKind::Error,
                    title: t!("admin-users-page-correction-failed"),
                    detail: "",
                    retry_label: t!("admin-retry"),
                    on_retry: retry_correction,
                }
            }
        }
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-users-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(&error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            rsx! {
            AdminUserTable {
                page,
                pagination,
                sort: query_state().sort,
                direction: query_state().direction,
                on_page_change: move |next_page| {
                    let mut next = query_state();
                    next.page = next_page;
                    next.apply(query_state);
                },
                on_sort_change: move |(sort, direction)| {
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    next.apply(query_state);
                },
                reload_generation,
                action_busy,
                action_feedback,
                create_notice,
                delivery,
            }
            }
        }
    };

    rsx! {
        section { class: "admin-users-page", aria_labelledby: "admin-users-title",
            header { class: "admin-users-heading",
                h1 { id: "admin-users-title", {t!("page-users-title")} }
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    disabled: command_gate.blocked() || pending.contains("create-user"),
                    onclick: move |_| {
                        if command_gate.blocked() || pending.contains("create-user") { return; }
                        action_feedback.set(None);
                        create_notice.set(None);
                        create_open.set(true);
                    },
                    {t!("admin-users-create-open")}
                }
            }
            if command_gate.blocked() {
                p { role: "status", {t!("admin-users-key-gate-help")} }
            }
            if create_open() {
                AdminUserCreateForm {
                    delivery,
                    open: create_open,
                    notice: create_notice,
                }
            }
            PendingCommandPanel { owner: pending }
            AdminUserFilters {
                applied: applied_filter,
                history_revision,
                on_name_apply: move |name: String| {
                    let mut next = query_state();
                    if next.name == name && next.page == 1 {
                        reload_generation.set(reload_generation().saturating_add(1));
                    } else {
                        next.name = name;
                        next.page = 1;
                        next.apply(query_state);
                    }
                },
                on_status_change: move |status: AdminUserStatusFilter| {
                    let mut next = query_state();
                    next.status = status;
                    next.page = 1;
                    next.apply(query_state);
                },
            }
            {content}
        }
        if let Some(key) = (delivery.generated)() {
            AdminUserGeneratedKeyDialog {
                generated: key,
                on_close: move |_| delivery.acknowledge(),
            }
        }
    }
}
