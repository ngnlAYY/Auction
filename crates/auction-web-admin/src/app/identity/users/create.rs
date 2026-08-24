//! Admin 用户创建表单和生成 key 展示。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::create_admin_user;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AdminUserGeneratedKey {
    pub(super) username: String,
    pub(super) key: String,
}

#[component]
pub(super) fn AdminUserCreateForm(
    reload_generation: Signal<u64>,
    on_created: EventHandler<Option<AdminUserGeneratedKey>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut username = use_signal(String::new);
    let mut game_player_id = use_signal(String::new);
    let mut key_mode = use_signal(|| "generated".to_owned());
    let mut manual_key = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);

    rsx! {
        div { class: "admin-users__dialog-backdrop", role: "presentation",
            section {
                class: "admin-users__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-users-create-title",
                aria_busy: busy().to_string(),
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        on_close.call(());
                    }
                },
                h2 { id: "admin-users-create-title", {t!("admin-users-create-title")} }
                form {
                    class: "admin-panel__actions admin-users__create-grid",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() {
                            return;
                        }
                        let username = username();
                        let game_player_id = game_player_id();
                        let key_mode = key_mode();
                        let manual_key = manual_key();
                        if username.trim().is_empty()
                            || game_player_id.trim().is_empty()
                            || (key_mode == "manual" && manual_key.is_empty())
                        {
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
                        let mut busy = busy;
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        let on_created = on_created;
                        busy.set(true);
                        spawn(async move {
                            let result = create_admin_user(
                                command_id,
                                username,
                                game_player_id,
                                if key_mode == "manual" { "manual" } else { "generated" },
                                (key_mode == "manual").then_some(manual_key),
                            )
                            .await;
                            match result {
                                Ok(result) => {
                                    let generated_key = result.generated_key.map(|key| {
                                        AdminUserGeneratedKey {
                                            username: result.user.username,
                                            key,
                                        }
                                    });
                                    on_created.call(generated_key);
                                    feedback.set(Some(true));
                                    command_id_state.set(None);
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
                            }
                            busy.set(false);
                        });
                    },
                    label {
                        r#for: "admin-users-username",
                        {t!("admin-users-username")}
                        input {
                            id: "admin-users-username",
                            r#type: "text",
                            required: true,
                            autofocus: true,
                            disabled: busy(),
                            value: username(),
                            oninput: move |event| {
                                username.set(event.value());
                                command_id_state.set(None);
                            },
                        }
                    }
                    label {
                        r#for: "admin-users-game-player-id",
                        {t!("admin-users-game-player-id")}
                        input {
                            id: "admin-users-game-player-id",
                            r#type: "text",
                            required: true,
                            disabled: busy(),
                            value: game_player_id(),
                            oninput: move |event| {
                                game_player_id.set(event.value());
                                command_id_state.set(None);
                            },
                        }
                    }
                    label {
                        r#for: "admin-users-key-mode",
                        {t!("admin-users-key-mode")}
                        select {
                            id: "admin-users-key-mode",
                            disabled: busy(),
                            value: key_mode(),
                            onchange: move |event| {
                                key_mode.set(event.value());
                                command_id_state.set(None);
                            },
                            option { value: "generated", {t!("admin-users-key-generated")} }
                            option { value: "manual", {t!("admin-users-key-manual")} }
                        }
                    }
                    if key_mode() == "manual" {
                        label {
                            r#for: "admin-users-manual-key",
                            {t!("admin-users-manual-key")}
                            input {
                                id: "admin-users-manual-key",
                                r#type: "password",
                                autocomplete: "new-password",
                                required: true,
                                disabled: busy(),
                                value: manual_key(),
                                oninput: move |event| {
                                    manual_key.set(event.value());
                                    command_id_state.set(None);
                                },
                            }
                        }
                    }
                    div { class: "admin-users__dialog-actions",
                        button {
                            r#type: "button",
                            class: "admin-btn-secondary",
                            disabled: busy(),
                            onclick: move |_| on_close.call(()),
                            {t!("admin-users-create-close")}
                        }
                        button {
                            r#type: "submit",
                            class: "admin-btn-primary",
                            disabled: busy(),
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-users-create-submit")} }
                        }
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
                        if success { {t!("admin-users-created")} } else { {t!("admin-users-action-error")} }
                    }
                }
            }
        }
    }
}

#[component]
pub(super) fn AdminUserGeneratedKeyDialog(
    generated: AdminUserGeneratedKey,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "admin-users__dialog-backdrop", role: "presentation",
            section {
                class: "admin-users__dialog admin-users__secret-dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-users-generated-key-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_close.call(());
                    }
                },
                h2 { id: "admin-users-generated-key-title", {t!("admin-users-generated-key-title")} }
                p { {t!("admin-users-generated-key-warning", username: generated.username)} }
                code { {generated.key} }
                div { class: "admin-users__dialog-actions",
                    button {
                        r#type: "button",
                        class: "admin-btn-primary",
                        autofocus: true,
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-generated-key-dismiss")}
                    }
                }
            }
        }
    }
}
