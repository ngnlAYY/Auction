//! 凭据页面的数据加载、删除编排和顶层布局。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use auction_web_shared::{AsyncState, AsyncStateKind};

use crate::api::{AdminCredential, delete_admin_credential, fetch_admin_credentials};

use super::super::query::use_page_query_state;
use super::super::{
    AdminActionFeedback, admin_api_error_message, run_admin_feedback_action, stable_command_id,
};
use super::{CredentialCreateForm, CredentialDeleteDialog, CredentialPager, CredentialTable};

/// 加载并展示 Admin 凭据管理页面。
pub fn page() -> Element {
    let mut reload_generation = use_signal(|| 0_u64);
    // 分页跟地址栏双向同步（?page=）；创建表单与删除确认对话框是瞬态，不入 URL。
    let query = use_page_query_state();
    let query_state = query.state;
    let mut pending_delete = use_signal(|| None::<AdminCredential>);
    let delete_busy = use_signal(|| false);
    let mut delete_feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut delete_command_state = use_signal(|| None::<(String, Uuid)>);
    let credentials = use_resource(move || {
        let _ = reload_generation();
        // 旧版凭据页没有列表筛选器，只按固定分页读取完整安全投影。
        fetch_admin_credentials(String::new(), String::new(), String::new(), query_state().page)
    });

    let open_delete = move |credential: AdminCredential| {
        if !delete_busy() {
            delete_feedback.set(None);
            pending_delete.set(Some(credential));
        }
    };
    let confirm_delete = move |credential: AdminCredential| {
        if delete_busy() {
            return;
        }
        let signature = format!("delete:{}:{}", credential.id, credential.state_version);
        let command_id = stable_command_id(delete_command_state, signature);
        run_admin_feedback_action(
            delete_busy,
            delete_feedback,
            async move { delete_admin_credential(command_id, &credential).await.map(|_| ()) },
            move |()| {
                pending_delete.set(None);
                delete_command_state.set(None);
                reload_generation.set(reload_generation().saturating_add(1));
            },
        );
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
                // 错误态提供页内重试入口，不必刷新浏览器。
                retry_label: t!("admin-retry"),
                on_retry: move |_| reload_generation.set(reload_generation().saturating_add(1)),
            }
        },
        Some(Ok(page)) => {
            let mut on_page_change = query.clone();
            let mut on_sort_change = query.clone();
            rsx! {
                    CredentialTable {
                        page: page.clone(),
                        sort: query_state().sort.clone(),
                        direction: query_state().direction,
                        on_sort_change: move |(sort, direction)| {
                            // 排序只影响当前页展示顺序；翻页保持不变。
                            let mut next = query_state();
                            next.sort = sort;
                            next.direction = direction;
                            on_sort_change.push(next);
                        },
                        reload_generation,
                        on_delete: open_delete,
                    }
                    CredentialPager {
                        page: page.clone(),
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
                credential,
                busy: delete_busy(),
                on_confirm: confirm_delete,
                on_cancel: move |_| pending_delete.set(None),
            }
        }
    }
}
