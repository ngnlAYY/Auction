//! 凭据页面的安全录入表单。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::create_admin_credential;
use crate::app::surface::{AdminActionFeedback, run_admin_feedback_action};

#[component]
pub fn CredentialCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut kind = use_signal(|| "mail_cookie".to_owned());
    let mut world = use_signal(|| "main".to_owned());
    let mut label = use_signal(String::new);
    let mut secret = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);

    rsx! {
        form {
            class: "admin-credentials-import",
            onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let mut kind_signal = kind;
                    let mut world_signal = world;
                    let mut label_signal = label;
                    let mut secret_signal = secret;
                    let kind = kind_signal();
                    let world = world_signal();
                    let label = label_signal();
                    let raw_secret = secret();
                    if label.trim().is_empty() || raw_secret.is_empty() {
                        feedback.set(Some(AdminActionFeedback::Error("invalid_request".to_owned())));
                        return;
                    }
                    let command_id = match command_id_state() {
                        Some(command_id) => command_id,
                        None => {
                            let command_id = Uuid::new_v4();
                            command_id_state.set(Some(command_id));
                            command_id
                        }
                    };
                    run_admin_feedback_action(
                        busy,
                        feedback,
                        async move {
                            create_admin_credential(command_id, kind, world, label, raw_secret)
                                .await
                                .map(|_| ())
                        },
                        move |()| {
                            kind_signal.set("mail_cookie".to_owned());
                            world_signal.set("main".to_owned());
                            label_signal.set(String::new());
                            secret_signal.set(String::new());
                            command_id_state.set(None);
                            reload_generation.set(reload_generation().saturating_add(1));
                        },
                    );
                },
            h2 { {t!("admin-credentials-create-title")} }
            label {
                    r#for: "import-kind",
                    {t!("admin-credentials-kind")}
                    select {
                        id: "import-kind",
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
                    r#for: "import-label",
                    {t!("admin-credentials-label")}
                    input {
                        id: "import-label",
                        r#type: "text",
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
                    r#for: "import-world",
                    {t!("admin-credentials-world")}
                    select {
                        id: "import-world",
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
                    r#for: "import-secret",
                    {t!("admin-credentials-secret")}
                    input {
                        id: "import-secret",
                        r#type: "password",
                        autocomplete: "off",
                        disabled: busy(),
                        required: true,
                        value: secret(),
                        oninput: move |event| {
                            secret.set(event.value());
                            command_id_state.set(None);
                        },
                    }
            }
            if let Some(feedback) = feedback() {
                    p {
                        class: if feedback.is_success() {
                            "admin-shell__inline-ok"
                        } else {
                            "admin-shell__inline-error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        {feedback.message(t!("admin-credentials-action-success"))}
                    }
            }
            button {
                    r#type: "submit",
                    class: "admin-btn-primary",
                    disabled: busy() || label().trim().is_empty() || secret().is_empty(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-credentials-create-submit")} }
            }
        }
    }
}
