//! 凭据删除确认对话框。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;

use crate::api::AdminCredential;

#[component]
pub fn CredentialDeleteDialog(
    credential: AdminCredential,
    busy: bool,
    on_confirm: EventHandler<AdminCredential>,
    on_cancel: EventHandler<()>,
) -> Element {
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管，删除确认的内容与按钮保持原样。
    rsx! {
        Dialog {
            title: t!("admin-credentials-delete-title"),
            close_label: t!("admin-credentials-cancel"),
            busy,
            on_close: move |_| on_cancel.call(()),
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
