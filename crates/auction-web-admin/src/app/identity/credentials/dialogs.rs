//! 凭据删除确认对话框。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};

use crate::api::AdminCredential;

#[component]
pub fn CredentialDeleteDialog(
    credential: AdminCredential,
    busy: bool,
    on_confirm: EventHandler<AdminCredential>,
    on_cancel: EventHandler<()>,
) -> Element {
    // 页面级删除遮罩可能与行内改名对话框同时存在，按动作区分标题 ID 可避免 aria 引用冲突。
    let title_id = format!("credential-delete-dialog-title-{}", credential.id);
    rsx! {
        div { class: "admin-shell__dialog-backdrop", role: "presentation",
            section {
                class: "admin-shell__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: title_id.clone(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy {
                        event.prevent_default();
                        event.stop_propagation();
                        on_cancel.call(());
                    }
                },
                h2 { id: title_id, {t!("admin-credentials-delete-title")} }
                p { {t!("admin-credentials-delete-warning", label: credential.label.clone())} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-secondary",
                        disabled: busy,
                        autofocus: true,
                        onclick: move |_| on_cancel.call(()),
                        {t!("admin-credentials-cancel")}
                    }
                    button {
                        r#type: "button",
                        class: "admin-btn-danger",
                        disabled: busy,
                        onclick: move |_| on_confirm.call(credential.clone()),
                        if busy { {t!("admin-action-working")} } else { {t!("admin-credentials-action-delete")} }
                    }
                }
            }
        }
    }
}
