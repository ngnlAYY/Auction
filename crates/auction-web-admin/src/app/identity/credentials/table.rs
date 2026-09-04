//! 凭据列表、状态操作和改名对话框入口。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminCredential, AdminCredentialPage, AdminSession, mutate_admin_credential};
use crate::app::surface::AdminActionFeedback;

use auction_web_shared::{AsyncState, AsyncStateKind, Pager, Time};

use super::{
    credential_kind_label, credential_status_class, credential_status_label,
    credential_world_label, run_action, stable_command_id,
};

#[component]
pub fn CredentialTable(
    page: AdminCredentialPage,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let session = use_context::<AdminSession>();
    let is_empty = page.credentials.is_empty();
    rsx! {
        div { class: "admin-credentials-table-wrap",
            table { class: "admin-credentials-table",
                thead {
                    tr {
                        th { {t!("admin-credentials-label")} }
                        th { {t!("admin-credentials-world")} }
                        th { {t!("admin-credentials-kind")} }
                        th { {t!("admin-credentials-status")} }
                        th { {t!("admin-credentials-last-checked")} }
                        th { {t!("admin-credentials-last-used")} }
                        th { {t!("admin-credentials-imported-by")} }
                        th { {t!("admin-credentials-actions")} }
                    }
                }
                tbody {
                    for credential in page.credentials {
                        CredentialRow {
                            credential,
                            imported_by: session.actor.clone(),
                            reload_generation,
                            on_delete,
                        }
                    }
                }
            }
        }
        if is_empty {
            AsyncState {
                kind: AsyncStateKind::Empty,
                title: t!("admin-credentials-empty"),
                detail: "",
            }
        }
    }
}

#[component]
pub fn CredentialRow(
    credential: AdminCredential,
    imported_by: String,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let mut rename_value = use_signal(|| credential.label.clone());
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<AdminActionFeedback>);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let credential_for_rename = credential.clone();
    let credential_for_rename_submit = credential.clone();
    let credential_for_enable = credential.clone();
    let credential_for_disable = credential.clone();
    let credential_for_delete = credential.clone();
    let feedback_id = format!("admin-credential-feedback-{}", credential.id);
    let rename_title_id = format!("credential-dialog-title-{}", credential.id);
    let rename_input_id = format!("label-{}", credential.id);
    rsx! {
        tr { class: "admin-credentials__row",
            td { "data-label": t!("admin-credentials-label"),
                h3 { class: "admin-visually-hidden", {credential.label.clone()} }
                TruncatedText { value: credential.label.clone() }
            }
            td { "data-label": t!("admin-credentials-world"), TruncatedText { value: credential_world_label(&credential.world), size: "compact" } }
            td { "data-label": t!("admin-credentials-kind"), {credential_kind_label(&credential.kind)} }
            td { "data-label": t!("admin-credentials-status"),
                span {
                    class: credential_status_class(&credential.status),
                    {credential_status_label(&credential.status)}
                }
            }
            td { "data-label": t!("admin-credentials-last-checked"),
                if let Some(rotated_at) = credential.rotated_at.clone() {
                    Time { value: rotated_at }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-credentials-last-used"), {t!("value-empty")} }
            td { "data-label": t!("admin-credentials-imported-by"),
                TruncatedText { value: imported_by }
            }
            td {
                "data-label": t!("admin-credentials-actions"),
                class: "admin-credentials__row-actions",
                aria_describedby: feedback().is_some().then_some(feedback_id.clone()),
                button {
                    r#type: "button",
                    disabled: busy() || credential.status == "active",
                    onclick: move |_| {
                        let credential = credential_for_enable.clone();
                        let status = "active";
                        let command_id = stable_command_id(
                            command_state,
                            format!("set_status:{}:{status}", credential.state_version),
                        );
                        run_action(
                            busy,
                            feedback,
                            reload_generation,
                            command_state,
                            async move {
                                mutate_admin_credential(
                                    command_id,
                                    &credential,
                                    "set_status",
                                    None,
                                    Some(status.to_owned()),
                                )
                                .await
                                .map(|_| ())
                            },
                        );
                    },
                    {t!("admin-credentials-action-enable")}
                }
                button {
                    r#type: "button",
                    disabled: busy() || credential.status == "disabled",
                    onclick: move |_| {
                        let credential = credential_for_disable.clone();
                        let status = "disabled";
                        let command_id = stable_command_id(
                            command_state,
                            format!("set_status:{}:{status}", credential.state_version),
                        );
                        run_action(
                            busy,
                            feedback,
                            reload_generation,
                            command_state,
                            async move {
                                mutate_admin_credential(
                                    command_id,
                                    &credential,
                                    "set_status",
                                    None,
                                    Some(status.to_owned()),
                                )
                                .await
                                .map(|_| ())
                            },
                        );
                    },
                    {t!("admin-credentials-action-disable")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        rename_value.set(credential_for_rename.label.clone());
                        rename_dialog.set(true);
                    },
                    {t!("admin-credentials-action-rename-open")}
                }
                button {
                    r#type: "button",
                    class: "admin-btn-danger",
                    disabled: busy(),
                    onclick: move |_| on_delete.call(credential_for_delete.clone()),
                    {t!("admin-credentials-action-delete")}
                }
                if let Some(feedback) = feedback() {
                    span {
                        id: feedback_id.clone(),
                        class: if feedback.is_success() {
                            "admin-credentials__feedback admin-credentials__feedback--success"
                        } else {
                            "admin-credentials__feedback admin-credentials__feedback--error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        aria_label: format!(
                            "{}: {}",
                            credential.label,
                            feedback.message(t!("admin-credentials-action-success")),
                        ),
                        {feedback.message(t!("admin-credentials-action-success"))}
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials-dialog-row",
                td { colspan: "8",
                    div { class: "admin-shell__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-shell__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: rename_title_id.clone(),
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    rename_dialog.set(false);
                                }
                            },
                            h2 { id: rename_title_id.clone(), {t!("admin-credentials-rename-title")} }
                            form {
                                onsubmit: move |event| {
                                    event.prevent_default();
                                    if busy() {
                                        return;
                                    }
                                    let credential = credential_for_rename_submit.clone();
                                    let label = rename_value();
                                    let command_id = stable_command_id(
                                        command_state,
                                        format!("rename:{}:{label}", credential.state_version),
                                    );
                                    let mut rename_dialog = rename_dialog;
                                    run_action(
                                        busy,
                                        feedback,
                                        reload_generation,
                                        command_state,
                                        async move {
                                            let result = mutate_admin_credential(
                                                command_id,
                                                &credential,
                                                "rename",
                                                Some(label),
                                                None,
                                            )
                                            .await
                                            .map(|_| ());
                                            if result.is_ok() {
                                                rename_dialog.set(false);
                                            }
                                            result
                                        },
                                    );
                                },
                                label {
                                    r#for: rename_input_id.clone(),
                                    {t!("admin-credentials-label")}
                                    input {
                                        id: rename_input_id.clone(),
                                        r#type: "text",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: rename_value(),
                                        oninput: move |event| rename_value.set(event.value()),
                                    }
                                }
                                div { class: "admin-shell__dialog-actions",
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: busy(),
                                        onclick: move |_| rename_dialog.set(false),
                                        {t!("admin-credentials-cancel")}
                                    }
                                    button {
                                        r#type: "submit",
                                        class: "admin-btn-primary",
                                        disabled: busy(),
                                        if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-rename-save")} }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn CredentialPager(page: AdminCredentialPage, page_number: Signal<i64>) -> Element {
    rsx! {
        Pager {
            page: page.page,
            page_size: page.page_size,
            total: Some(page.total),
            label: t!("page-credentials-title"),
            previous_label: t!("admin-pagination-previous"),
            status: move |(page, pages): (i64, i64)| {
                t!("admin-pagination-status", page: page, pages: pages)
            },
            next_label: t!("admin-pagination-next"),
            jump_label: t!("admin-pagination-input"),
            jump_button_label: t!("admin-pagination-go"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
