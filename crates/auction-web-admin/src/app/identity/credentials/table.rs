//! 凭据列表、状态操作和改名对话框入口。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminCredential, AdminCredentialPage, mutate_admin_credential};

use super::super::{format_admin_time, pagination::AdminPager};
use super::{
    credential_kind_label, credential_status_action_label, credential_status_class,
    credential_status_label, credential_world_label, run_action, stable_command_id,
};

#[component]
pub fn CredentialTable(
    page: AdminCredentialPage,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    rsx! {
        section {
            class: "admin-credentials__list",
            aria_labelledby: "admin-credentials-title",
            p { class: "admin-panel__note", {t!("admin-credentials-summary", total: page.total)} }
            if page.restart_required {
                p { class: "admin-panel__note", {t!("admin-credentials-restart-note")} }
            }
            if page.credentials.is_empty() {
                p { class: "admin-panel__note", {t!("admin-credentials-empty")} }
            } else {
                div { class: "public-auction-catalog admin-credentials__table-wrap",
                    table {
                        thead {
                            tr {
                                th { {t!("admin-credentials-kind")} }
                                th { {t!("admin-credentials-world")} }
                                th { {t!("admin-credentials-label")} }
                                th { {t!("admin-credentials-status")} }
                                th { {t!("admin-credentials-rotated-at")} }
                                th { {t!("admin-credentials-version")} }
                                th { {t!("admin-credentials-actions")} }
                            }
                        }
                        tbody {
                            for credential in page.credentials {
                                CredentialRow { credential, reload_generation, on_delete }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn CredentialRow(
    credential: AdminCredential,
    reload_generation: Signal<u64>,
    on_delete: EventHandler<AdminCredential>,
) -> Element {
    let mut rename_value = use_signal(|| credential.label.clone());
    let mut rename_dialog = use_signal(|| false);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let credential_for_rename = credential.clone();
    let credential_for_rename_submit = credential.clone();
    let credential_for_status = credential.clone();
    let credential_for_delete = credential.clone();
    let next_status = match credential.status.as_str() {
        "active" => Some("disabled"),
        "disabled" => Some("active"),
        _ => None,
    };
    rsx! {
        tr { class: "admin-credentials__row",
            td { "data-label": t!("admin-credentials-kind"), {credential_kind_label(&credential.kind)} }
            td { "data-label": t!("admin-credentials-world"), TruncatedText { value: credential_world_label(&credential.world), size: "compact" } }
            td { "data-label": t!("admin-credentials-label"), TruncatedText { value: credential.label.clone() } }
            td { "data-label": t!("admin-credentials-status"),
                span {
                    class: credential_status_class(&credential.status),
                    {credential_status_label(&credential.status)}
                }
            }
            td { "data-label": t!("admin-credentials-rotated-at"),
                if let Some(rotated_at) = credential.rotated_at.clone() {
                    time { datetime: rotated_at.clone(), "{format_admin_time(&rotated_at)}" }
                } else {
                    {t!("value-empty")}
                }
            }
            td {
                "data-label": t!("admin-credentials-version"),
                class: "public-auction-catalog__number",
                "{credential.state_version}"
            }
            td { "data-label": t!("admin-credentials-actions"), class: "admin-credentials__row-actions",
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        rename_value.set(credential_for_rename.label.clone());
                        rename_dialog.set(true);
                    },
                    {t!("admin-credentials-action-rename-open")}
                }
                if let Some(status) = next_status {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            let credential = credential_for_status.clone();
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
                        {credential_status_action_label(&credential.status)}
                    }
                }
                button {
                    r#type: "button",
                    class: "admin-btn-danger",
                    disabled: busy(),
                    onclick: move |_| on_delete.call(credential_for_delete.clone()),
                    {t!("admin-credentials-action-delete")}
                }
                if let Some(success) = feedback() {
                    span {
                        class: if success {
                            "admin-credentials__feedback admin-credentials__feedback--success"
                        } else {
                            "admin-credentials__feedback admin-credentials__feedback--error"
                        },
                        if success { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                    }
                }
            }
        }
        if rename_dialog() {
            tr { class: "admin-credentials__dialog-row",
                td { colspan: "7",
                    div { class: "admin-credentials__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-credentials__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-credentials-rename-title",
                            aria_busy: busy().to_string(),
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    rename_dialog.set(false);
                                }
                            },
                            h2 { id: "admin-credentials-rename-title", {t!("admin-credentials-rename-title")} }
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
                                    {t!("admin-credentials-label")}
                                    input {
                                        r#type: "text",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: rename_value(),
                                        oninput: move |event| rename_value.set(event.value()),
                                    }
                                }
                                div { class: "admin-credentials__dialog-actions",
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
        AdminPager {
            page: page.page,
            page_size: page.page_size,
            total: page.total,
            label: t!("admin-credentials-pagination"),
            on_page_change: move |next_page| page_number.set(next_page),
        }
    }
}
