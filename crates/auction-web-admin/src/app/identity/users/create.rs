//! Admin 用户创建表单和生成 key 展示。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::create_admin_user;
use crate::app::surface::{AdminActionFeedback, run_admin_action};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AdminUserGeneratedKey {
    pub(super) key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AdminUserCreated {
    pub(super) username: String,
    pub(super) generated_key: Option<AdminUserGeneratedKey>,
}

#[component]
pub(super) fn AdminUserCreateForm(
    reload_generation: Signal<u64>,
    on_created: EventHandler<AdminUserCreated>,
    on_close: EventHandler<()>,
) -> Element {
    let mut username = use_signal(String::new);
    let mut forum_account_id = use_signal(String::new);
    let mut key_mode = use_signal(|| "generated".to_owned());
    let mut manual_key = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<(Uuid, Uuid)>);
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<AdminActionFeedback>);

    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；同时修正旧版 aria_labelledby
    // 指向不存在 id（admin-users-create-title vs create-user-title）的脱节问题。
    rsx! {
        Dialog {
            title: t!("admin-users-create-title"),
            close_label: t!("admin-users-create-close"),
            busy: busy(),
            on_close: move |_| on_close.call(()),
            form {
                    class: "admin-users-create",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if busy() {
                            return;
                        }
                        let username = username().trim().to_owned();
                        let forum_account_id = forum_account_id().trim().to_owned();
                        let key_mode = key_mode();
                        let submitted_manual_key = manual_key();
                        if username.is_empty()
                            || (key_mode == "manual" && submitted_manual_key.is_empty())
                        {
                            return;
                        }
                        let signature = create_signature(
                            &username,
                            &forum_account_id,
                            &key_mode,
                            &submitted_manual_key,
                        );
                        let command_id = match command_id_state() {
                            Some((current_signature, command_id))
                                if current_signature == signature =>
                            {
                                command_id
                            }
                            None => {
                                let command_id = Uuid::new_v4();
                                command_id_state.set(Some((signature, command_id)));
                                command_id
                            }
                            Some(_) => {
                                let command_id = Uuid::new_v4();
                                command_id_state.set(Some((signature, command_id)));
                                command_id
                            }
                        };
                        if key_mode == "manual" {
                            manual_key.set(String::new());
                        }
                        let mut feedback = feedback;
                        let mut reload_generation = reload_generation;
                        let on_created = on_created;
                        feedback.set(None);
                        run_admin_action(
                            busy,
                            async move {
                                create_admin_user(
                                    command_id,
                                    username,
                                    (!forum_account_id.is_empty()).then_some(forum_account_id),
                                    if key_mode == "manual" { "manual" } else { "generated" },
                                    (key_mode == "manual").then_some(submitted_manual_key),
                                )
                                .await
                            },
                            move |result| {
                                let username = result.user.username;
                                let generated_key =
                                    result.generated_key.map(|key| AdminUserGeneratedKey { key });
                                on_created.call(AdminUserCreated { username, generated_key });
                                command_id_state.set(None);
                                reload_generation.set(reload_generation().saturating_add(1));
                            },
                            move |code| feedback.set(Some(AdminActionFeedback::Error(code))),
                        );
                    },
                    label { r#for: "create-username", {t!("admin-users-username")} }
                    input {
                        id: "create-username",
                        r#type: "text",
                        required: true,
                        value: username(),
                        oninput: move |event| username.set(event.value()),
                    }
                    label { r#for: "create-forum-account-id", {t!("admin-users-forum-account-id")} }
                    input {
                        id: "create-forum-account-id",
                        r#type: "text",
                        inputmode: "numeric",
                        pattern: "[1-9][0-9]*",
                        aria_describedby: "forum-account-id-help",
                        value: forum_account_id(),
                        oninput: move |event| forum_account_id.set(event.value()),
                    }
                    p { id: "forum-account-id-help", {t!("admin-users-forum-account-id-help")} }
                    label { r#for: "create-key-mode", {t!("admin-users-key-mode")} }
                    select {
                        id: "create-key-mode",
                        value: key_mode(),
                        onchange: move |event| {
                            key_mode.set(event.value());
                            manual_key.set(String::new());
                        },
                        option { value: "generated", {t!("admin-users-key-generated")} }
                        option { value: "manual", {t!("admin-users-key-manual")} }
                    }
                    if key_mode() == "manual" {
                        label { r#for: "create-key", {t!("admin-users-manual-key")} }
                        input {
                            id: "create-key",
                            r#type: "password",
                            autocomplete: "new-password",
                            minlength: "32",
                            required: true,
                            value: manual_key(),
                            oninput: move |event| manual_key.set(event.value()),
                        }
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: busy(),
                        {t!("admin-users-create-submit")}
                    }
                }
                if let Some(AdminActionFeedback::Error(code)) = feedback() {
                    p { role: "alert", {AdminActionFeedback::Error(code).message(String::new())} }
                }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-create-close")}
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
        // 一次性密钥展示弹窗：外壳由共享 Dialog 接管；.admin-users-secret 内容网格
        // 样式移到面板内的包装 div。
        Dialog {
            title: t!("admin-users-generated-key-title"),
            close_label: t!("admin-users-generated-key-dismiss"),
            on_close: move |_| on_close.call(()),
            div { class: "admin-users-secret",
                p { {t!("admin-users-generated-key-warning")} }
                code { {generated.key} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("admin-users-generated-key-dismiss")}
                    }
                }
            }
        }
    }
}

fn create_signature(
    username: &str,
    forum_account_id: &str,
    key_mode: &str,
    manual_key: &str,
) -> Uuid {
    let mut payload = Vec::new();
    for value in [username, forum_account_id, key_mode, manual_key] {
        payload.extend_from_slice(&(value.len() as u64).to_be_bytes());
        payload.extend_from_slice(value.as_bytes());
    }
    Uuid::new_v5(&Uuid::NAMESPACE_OID, &payload)
}
