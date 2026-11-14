//! 凭据页面的数据加载、删除编排和顶层布局。

use auction_web_shared::t;
use dioxus::prelude::*;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminCredential, delete_admin_credential, fetch_admin_credentials};

use super::super::query::use_page_query_state;
use super::super::{AdminActionFeedback, admin_api_error_message};
use super::{CredentialCreateForm, CredentialDeleteDialog, CredentialTable};
use crate::surface::pagination::{AdminPager, use_admin_page, use_admin_pagination};
use crate::surface::pending::{PendingCommandPanel, set_if_alive, use_pending_commands};

/// 加载并展示 Admin 凭据管理页面。
pub fn page() -> Element {
    let mut reload_generation = use_signal(|| 0_u64);
    let pending_commands = use_pending_commands(reload_generation);
    // 分页跟地址栏双向同步（?page=）；创建表单与删除确认对话框是瞬态，不入 URL。
    let query = use_page_query_state();
    let query_state = query.state;
    let requested_page = use_memo(move || query_state.read().page);
    let mut pending_delete = use_signal(|| None::<AdminCredential>);
    let mut delete_feedback = use_signal(|| None::<AdminActionFeedback>);
    let pagination =
        use_admin_pagination("credentials", &["page_size"], Some(200), i64::from(i32::MAX));
    let credentials = use_admin_page(
        pagination.clone(),
        move || {
            let _ = reload_generation();
            requested_page()
        },
        |page, size| {
            fetch_admin_credentials(String::new(), String::new(), String::new(), page, size)
        },
    );

    let open_delete = move |credential: AdminCredential| {
        if !pending_commands.contains(&format!("credential:{}", credential.id)) {
            delete_feedback.set(None);
            pending_delete.set(Some(credential));
        }
    };
    let confirm_delete = move |credential: AdminCredential| {
        if pending_commands.contains(&format!("credential:{}", credential.id)) {
            return;
        }
        delete_feedback.set(None);
        // 删除与行内变更共用实体门禁；关闭确认框不能丢弃未知结果的原 UUID 和版本。
        if pending_commands.submit(
            format!("credential:{}", credential.id),
            t!("admin-credentials-action-delete"),
            move |command_id| {
                let credential = credential.clone();
                async move { delete_admin_credential(command_id, &credential).await.map(|_| ()) }
            },
            move |()| set_if_alive(delete_feedback, Some(AdminActionFeedback::Success)),
            move |code| set_if_alive(delete_feedback, Some(AdminActionFeedback::Error(code))),
        ) {
            pending_delete.set(None);
        }
    };

    let list_content = match &*credentials.read_unchecked() {
        None => rsx! {
            AsyncState {
                kind: AsyncStateKind::Loading,
                title: t!("admin-credentials-loading"),
                detail: "",
            }
        },
        Some(Err(error)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: admin_api_error_message(error),
                detail: "",
                retry_label: t!("admin-retry"),
                on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query;
            rsx! {
                    CredentialTable {
                        page: page.clone(),
                        sort: query_state().sort,
                        direction: query_state().direction,
                        on_sort_change: move |(sort, direction)| {
                            let mut next = query_state();
                            next.sort = sort;
                            next.direction = direction;
                            on_sort_change.push(next);
                        },
                        on_delete: open_delete,
                    }
                    AdminPager {
                        page: page.page,
                        page_size: page.page_size,
                        total: Some(page.total),
                        pagination,
                        label: t!("page-credentials-title"),
                        on_page_change: move |next_page| {
                            let mut next = query_state();
                            next.page = next_page;
                            on_page_change.push(next);
                        },
                    }
            }
        }
    };
    let pending = pending_delete();

    rsx! {
        section {
            class: "admin-credentials-page",
            aria_labelledby: "credentials-title",
            h1 { id: "credentials-title", {t!("page-credentials-title")} }
            CredentialCreateForm { reload_generation }
            PendingCommandPanel { owner: pending_commands }
            {list_content}
            if let Some(feedback) = delete_feedback() {
                p {
                    class: if feedback.is_success() {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if feedback.is_success() { "status" } else { "alert" },
                    {feedback.message(t!("admin-credentials-action-success"))}
                }
            }
        }
        if let Some(credential) = pending {
            CredentialDeleteDialog {
                busy: pending_commands.is_busy(&format!("credential:{}", credential.id)),
                credential,
                on_confirm: confirm_delete,
                on_cancel: move |_| pending_delete.set(None),
            }
        }
    }
}
