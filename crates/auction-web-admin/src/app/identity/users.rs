//! Admin 用户管理页面。
//!
//! 页面只维护筛选、表单和安全用户投影；所有改变状态的动作都由后端命令 API 生成
//! command_id，页面会在同一输入重试时复用它，并使用返回的 state_version。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{AdminUserStatusFilter, fetch_admin_users};

use super::{AdminLoadError, AdminPanel};

#[path = "users/create.rs"]
mod create;
#[path = "users/dialogs.rs"]
mod dialogs;
#[path = "users/row.rs"]
mod row;
#[path = "users/table.rs"]
mod table;

use create::{AdminUserCreateForm, AdminUserGeneratedKey, AdminUserGeneratedKeyDialog};
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
    let users = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_users(name_filter(), status_filter(), page_number())
    });

    match &*users.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-users-title"), detail: t!("admin-users-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-users-title"),
                detail: t!("admin-users-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => rsx! {
            div { class: "admin-users",
                header { class: "admin-users__heading",
                    h1 { id: "admin-users-title", {t!("page-users-title")} }
                    button {
                        class: "admin-users__create-opener admin-btn-primary",
                        r#type: "button",
                        onclick: move |_| create_open.set(true),
                        {t!("admin-users-create-open")}
                    }
                }
                if create_open() {
                    AdminUserCreateForm {
                        reload_generation,
                        on_created: move |key| {
                            generated_key.set(key);
                            create_open.set(false);
                        },
                        on_close: move |_| create_open.set(false),
                    }
                }
                AdminUserTable {
                    page: page.clone(),
                    name_filter,
                    name_draft,
                    status_filter,
                    page_number,
                    reload_generation,
                    on_generated_key: move |key| generated_key.set(Some(key)),
                }
            }
            if let Some(key) = generated_key() {
                AdminUserGeneratedKeyDialog {
                    generated: key,
                    on_close: move |_| generated_key.set(None),
                }
            }
        },
    }
}
