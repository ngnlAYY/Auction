//! Admin 用户管理页面。
//! 页面只维护筛选、表单和安全用户投影；页级命令宿主冻结完整负载和 command_id，
//! 原样重试并使用返回的 state_version，一次性 Key 由稳定页面接收。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminUserStatusFilter, fetch_admin_users};

use auction_web_shared::{AsyncState, AsyncStateKind};

use super::pagination::{use_admin_page, use_admin_pagination};
use super::pending::{
    PendingCommandPanel, use_pending_command_gate, use_pending_commands_with_gate,
};
use super::query::use_users_query_state;
use super::{AdminActionFeedback, admin_api_error_message};

mod create;
mod dialogs;
mod row;
mod table;

use create::{
    AdminUserCreateForm, AdminUserCreated, AdminUserGeneratedKey, AdminUserGeneratedKeyDialog,
};
use table::AdminUserTable;

/// 加载并展示 Admin 用户管理页面。
pub(super) fn page() -> Element {
    // 已应用的名称筛选、状态筛选与分页跟地址栏双向同步；名称输入框草稿只在提交时入 URL。
    let query = use_users_query_state();
    let query_state = query.state;
    let requested_query = use_memo(move || {
        let current = query_state.read();
        (current.name.clone(), current.status, current.page)
    });
    // 草稿初值取自地址栏恢复的已应用筛选，刷新后表单与列表保持一致；之后的编辑不落 URL。
    let name_draft = use_signal(move || query_state.peek().name.clone());
    let mut reload_generation = use_signal(|| 0_u64);
    let mut create_open = use_signal(|| false);
    let mut generated_key = use_signal(|| None::<AdminUserGeneratedKey>);
    let mut action_busy = use_signal(|| false);
    let command_gate = use_pending_command_gate();
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
        |(name, status, page), size| fetch_admin_users(name, status, page, size),
    );

    let state = users.read_unchecked().clone();
    let content = match state {
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
            let mut on_name_apply = query.clone();
            let mut on_status_change = query.clone();
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
            AdminUserTable {
                page,
                pagination,
                name_draft,
                status: query_state().status,
                sort: query_state().sort,
                direction: query_state().direction,
                on_name_apply: move |name: String| {
                    let mut next = query_state();
                    next.name = name;
                    next.page = 1;
                    on_name_apply.push(next);
                },
                on_status_change: move |status: AdminUserStatusFilter| {
                    let mut next = query_state();
                    next.status = status;
                    next.page = 1;
                    on_status_change.push(next);
                },
                on_page_change: move |next_page| {
                    let mut next = query_state();
                    next.page = next_page;
                    on_page_change.push(next);
                },
                on_sort_change: move |(sort, direction)| {
                    let mut next = query_state();
                    next.sort = sort;
                    next.direction = direction;
                    on_sort_change.push(next);
                },
                reload_generation,
                action_busy,
                action_feedback,
                create_notice,
                on_generated_key: move |key| {
                    command_gate.hold_result();
                    generated_key.set(Some(key));
                },
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
                    command_gate,
                    on_created: move |created: AdminUserCreated| {
                        create_notice.set(Some(t!(
                            "admin-users-created",
                            username: created.username
                        )));
                        if let Some(key) = created.generated_key {
                            command_gate.hold_result();
                            generated_key.set(Some(key));
                        }
                        create_open.set(false);
                    },
                    on_close: move |_| create_open.set(false),
                }
            }
            PendingCommandPanel { owner: pending }
            {content}
        }
        if let Some(key) = generated_key() {
            AdminUserGeneratedKeyDialog {
                generated: key,
                on_close: move |_| {
                    generated_key.set(None);
                    command_gate.acknowledge();
                },
            }
        }
    }
}
