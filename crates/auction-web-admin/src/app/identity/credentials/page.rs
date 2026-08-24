//! 凭据页面的数据加载、删除编排和顶层布局。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{AdminCredential, delete_admin_credential, fetch_admin_credentials};

use super::super::{AdminLoadError, AdminPanel};
use super::{
    CredentialCreateForm, CredentialDeleteDialog, CredentialFilters, CredentialPager,
    CredentialTable, stable_command_id,
};

/// 加载并展示 Admin 凭据管理页面。
pub fn page() -> Element {
    let reload_generation = use_signal(|| 0_u64);
    let kind_filter = use_signal(String::new);
    let world_filter = use_signal(String::new);
    let status_filter = use_signal(String::new);
    let page_number = use_signal(|| 1_i64);
    let mut pending_delete = use_signal(|| None::<AdminCredential>);
    let mut delete_busy = use_signal(|| false);
    let mut delete_feedback = use_signal(|| None::<bool>);
    let mut delete_command_state = use_signal(|| None::<(String, Uuid)>);
    let credentials = use_resource(move || {
        let _ = reload_generation();
        fetch_admin_credentials(kind_filter(), world_filter(), status_filter(), page_number())
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
        delete_feedback.set(None);
        delete_busy.set(true);
        let signature = format!("delete:{}:{}", credential.id, credential.state_version);
        let command_id = stable_command_id(delete_command_state, signature);
        let mut delete_busy = delete_busy;
        let mut delete_feedback = delete_feedback;
        let mut pending_delete = pending_delete;
        let mut reload_generation = reload_generation;
        spawn(async move {
            let successful = delete_admin_credential(command_id, &credential).await.is_ok();
            delete_feedback.set(Some(successful));
            delete_busy.set(false);
            if successful {
                pending_delete.set(None);
                delete_command_state.set(None);
                reload_generation.set(reload_generation().saturating_add(1));
            }
        });
    };

    match &*credentials.read_unchecked() {
        None => rsx! {
            AdminPanel { title: t!("page-credentials-title"), detail: t!("admin-credentials-loading") }
        },
        Some(Err(_)) => rsx! {
            AdminLoadError {
                title: t!("page-credentials-title"),
                detail: t!("admin-credentials-error"),
                refresh: reload_generation,
            }
        },
        Some(Ok(page)) => {
            let pending = pending_delete();
            rsx! {
                div { class: "admin-credentials",
                    h1 { id: "admin-credentials-title", {t!("page-credentials-title")} }
                    CredentialCreateForm { reload_generation }
                    CredentialFilters { kind_filter, world_filter, status_filter, page_number }
                    CredentialTable {
                        page: page.clone(),
                        reload_generation,
                        on_delete: open_delete,
                    }
                    CredentialPager { page: page.clone(), page_number }
                }
                if let Some(credential) = pending {
                    CredentialDeleteDialog {
                        credential,
                        busy: delete_busy(),
                        on_confirm: confirm_delete,
                        on_cancel: move |_| pending_delete.set(None),
                    }
                }
                if let Some(successful) = delete_feedback() {
                    p {
                        class: if successful {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if successful { "status" } else { "alert" },
                        if successful { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                    }
                }
            }
        }
    }
}
