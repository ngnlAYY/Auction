//! 凭据删除确认对话框。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;

use crate::api::AdminCredential;

#[component]
pub fn CredentialDeleteDialog(
    credential: AdminCredential,
    busy: bool,
    on_confirm: EventHandler<AdminCredential>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-credentials__dialog-backdrop", role: "presentation",
            section {
                class: "admin-credentials__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-credentials-delete-title",
                aria_busy: busy.to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy {
                        on_cancel.call(());
                    }
                },
                h2 { id: "admin-credentials-delete-title", {t!("admin-credentials-delete-title")} }
                p { {t!("admin-credentials-delete-warning", label: credential.label.clone())} }
                div { class: "admin-credentials__dialog-actions",
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
