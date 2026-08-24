//! 凭据页面的筛选和安全录入表单。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::create_admin_credential;

#[component]
pub fn CredentialFilters(
    mut kind_filter: Signal<String>,
    mut world_filter: Signal<String>,
    mut status_filter: Signal<String>,
    mut page_number: Signal<i64>,
) -> Element {
    rsx! {
        section { class: "admin-credentials__filters", aria_labelledby: "admin-credentials-filter-title",
            h2 { id: "admin-credentials-filter-title", {t!("admin-credentials-filter")} }
            div { class: "admin-panel__actions",
                label {
                    {t!("admin-credentials-kind")}
                    select {
                        value: kind_filter(),
                        onchange: move |event| {
                            kind_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "mail_cookie", {t!("admin-credentials-kind-mail")} }
                        option { value: "forum_cookie", {t!("admin-credentials-kind-forum")} }
                    }
                }
                label {
                    {t!("admin-credentials-world")}
                    select {
                        value: world_filter(),
                        onchange: move |event| {
                            world_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "main", {t!("admin-credentials-world-main")} }
                        option { value: "isekai", {t!("admin-credentials-world-isekai")} }
                        option { value: "shared", {t!("admin-credentials-world-shared")} }
                    }
                }
                label {
                    {t!("admin-credentials-status")}
                    select {
                        value: status_filter(),
                        onchange: move |event| {
                            status_filter.set(event.value());
                            page_number.set(1);
                        },
                        option { value: "", {t!("admin-credentials-all")} }
                        option { value: "active", {t!("admin-credentials-status-active")} }
                        option { value: "disabled", {t!("admin-credentials-status-disabled")} }
                        option { value: "invalid", {t!("admin-credentials-status-invalid")} }
                    }
                }
            }
        }
    }
}

#[component]
pub fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut kind = use_signal(|| "mail_cookie".to_owned());
    let mut world = use_signal(|| "main".to_owned());
    let mut label = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let mut busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<bool>);

    rsx! {
        section {
            class: "admin-credentials__import",
            aria_labelledby: "admin-credentials-create-title",
            aria_busy: busy().to_string(),
            h2 { id: "admin-credentials-create-title", {t!("admin-credentials-create-title")} }
            p { class: "admin-panel__note", {t!("admin-credentials-secret-note")} }
            form {
                class: "admin-credentials__create-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let kind = kind();
                    let world = world();
                    let label = label();
                    let raw_secret = secret();
                    if label.trim().is_empty() || raw_secret.is_empty() {
                        feedback.set(Some(false));
                        return;
                    }
                    let mut secret_signal = secret;
                    let command_id = match command_id_state() {
                        Some(command_id) => command_id,
                        None => {
                            let command_id = Uuid::new_v4();
                            command_id_state.set(Some(command_id));
                            command_id
                        }
                    };
                    busy.set(true);
                    feedback.set(None);
                    let mut busy = busy;
                    let mut feedback = feedback;
                    let mut reload_generation = reload_generation;
                    spawn(async move {
                        let result =
                            create_admin_credential(command_id, kind, world, label, raw_secret).await;
                        match result {
                            Ok(_) => {
                                secret_signal.set(String::new());
                                command_id_state.set(None);
                                feedback.set(Some(true));
                                reload_generation.set(reload_generation().saturating_add(1));
                            }
                            Err(_) => feedback.set(Some(false)),
                        }
                        busy.set(false);
                    });
                },
                label {
                    r#for: "admin-credentials-kind",
                    {t!("admin-credentials-kind")}
                    select {
                        id: "admin-credentials-kind",
                        disabled: busy(),
                        value: kind(),
                        onchange: move |event| {
                            kind.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "mail_cookie", {t!("admin-credentials-kind-mail")} }
                        option { value: "forum_cookie", {t!("admin-credentials-kind-forum")} }
                    }
                }
                label {
                    r#for: "admin-credentials-world",
                    {t!("admin-credentials-world")}
                    select {
                        id: "admin-credentials-world",
                        disabled: busy(),
                        value: world(),
                        onchange: move |event| {
                            world.set(event.value());
                            command_id_state.set(None);
                        },
                        option { value: "main", {t!("admin-credentials-world-main")} }
                        option { value: "isekai", {t!("admin-credentials-world-isekai")} }
                        option { value: "shared", {t!("admin-credentials-world-shared")} }
                    }
                }
                label {
                    r#for: "admin-credentials-label",
                    {t!("admin-credentials-label")}
                    input {
                        id: "admin-credentials-label",
                        disabled: busy(),
                        required: true,
                        value: label(),
                        oninput: move |event| {
                            label.set(event.value());
                            command_id_state.set(None);
                        },
                    }
                }
                label {
                    r#for: "admin-credentials-secret",
                    {t!("admin-credentials-secret")}
                    input {
                        id: "admin-credentials-secret",
                        r#type: "password",
                        autocomplete: "new-password",
                        disabled: busy(),
                        required: true,
                        value: secret(),
                        oninput: move |event| {
                            secret.set(event.value());
                            command_id_state.set(None);
                        },
                    }
                }
                button {
                    r#type: "submit",
                    disabled: busy() || label().trim().is_empty() || secret().is_empty(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-create-submit")} }
                }
            }
            if let Some(success) = feedback() {
                p {
                    class: if success {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if success { "status" } else { "alert" },
                    if success { {t!("admin-credentials-action-success")} } else { {t!("admin-credentials-action-error")} }
                }
            }
        }
    }
}
