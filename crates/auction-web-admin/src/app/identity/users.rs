//! Admin 用户管理页面。
//!
//! 页面只维护筛选、表单和安全用户投影；所有改变状态的动作都由后端命令 API 生成
//! command_id，页面会在同一输入重试时复用它，并使用返回的 state_version。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{AdminUserStatusFilter, fetch_admin_users};

use auction_web_shared::{AsyncState, AsyncStateKind};

use super::{AdminActionFeedback, admin_api_error_message};

#[path = "users/create.rs"]
mod create;
#[path = "users/dialogs.rs"]
mod dialogs;
#[path = "users/row.rs"]
mod row;
#[path = "users/table.rs"]
mod table;

use create::{
    AdminUserCreateForm, AdminUserCreated, AdminUserGeneratedKey, AdminUserGeneratedKeyDialog,
};
use table::AdminUserTable;

/// 加载并展示 Admin 用户管理页面。
pub(super) fn page() -> Element {
    let name_filter = use_signal(String::new);
    let name_draft = use_signal(String::new);
    let status_filter = use_signal(|| AdminUserStatusFilter::All);
    let page_number = use_signal(|| 1_i64);
    let reload_generation = use_signal(|| 0_u64);
    let mut create_open = use_signal(|| false);
    let mut generated_key = use_signal(|| None::<AdminUserGeneratedKey>);
    let action_busy = use_signal(|| false);
    let mut action_feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut create_notice = use_signal(|| None::<String>);
    let users = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_users(name_filter(), status_filter(), page_number())
    });

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
            }
        },
        Some(Ok(page)) => rsx! {
            AdminUserTable {
                page,
                name_filter,
                name_draft,
                status_filter,
                page_number,
                reload_generation,
                action_busy,
                action_feedback,
                create_notice,
                on_generated_key: move |key| generated_key.set(Some(key)),
            }
        },
    };

    rsx! {
        section { class: "admin-users-page", aria_labelledby: "admin-users-title",
            header { class: "admin-users-heading",
                h1 { id: "admin-users-title", {t!("page-users-title")} }
                button {
                    class: "admin-btn-primary",
                    r#type: "button",
                    onclick: move |_| {
                        action_feedback.set(None);
                        create_notice.set(None);
                        create_open.set(true);
                    },
                    {t!("admin-users-create-open")}
                }
            }
            if create_open() {
                AdminUserCreateForm {
                    reload_generation,
                    on_created: move |created: AdminUserCreated| {
                        create_notice.set(Some(t!(
                            "admin-users-created",
                            username: created.username
                        )));
                        generated_key.set(created.generated_key);
                        create_open.set(false);
                    },
                    on_close: move |_| create_open.set(false),
                }
            }
            {content}
        }
        if let Some(key) = generated_key() {
            AdminUserGeneratedKeyDialog {
                generated: key,
                on_close: move |_| generated_key.set(None),
            }
        }
    }
}
