//! Admin 用户行展示和用户状态操作。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use auction_web_shared::TruncatedText;

use crate::api::{AdminUser, blacklist_admin_user, mutate_admin_user, unblacklist_admin_user};

use super::super::format_admin_time;
use super::create::AdminUserGeneratedKey;
use super::dialogs::{
    AdminUserDialogActions, AdminUserDialogKind, run_user_action, stable_command_id,
    user_key_status_class, user_key_status_label, user_status_class, user_status_label,
};

#[component]
pub(super) fn AdminUserRow(
    user: AdminUser,
    reload_generation: Signal<u64>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<String>);
    let mut rename_value = use_signal(|| user.username.clone());
    let mut game_player_id_value = use_signal(|| user.game_player_id.clone().unwrap_or_default());
    let mut dialog = use_signal(|| None::<AdminUserDialogKind>);
    let mut dialog_reason = use_signal(String::new);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);
    let user_for_rename = user.clone();
    let user_for_game_dialog = user.clone();
    let user_for_revoke = user.clone();
    let user_for_reissue = user.clone();
    let user_for_blacklist = user.clone();
    let user_for_action = user.clone();
    let dialog_title = dialog().map(|kind| kind.title(&user.username));
    rsx! {
        tr { class: "admin-users__row",
            td { "data-label": t!("admin-users-username"), TruncatedText { value: user.username.clone() } }
            td { "data-label": t!("admin-users-game-player-id"), TruncatedText { value: user.game_player_id.clone().unwrap_or_else(|| t!("admin-users-empty")), size: "wide" } }
            td { "data-label": t!("admin-users-status"),
                span {
                    class: user_status_class(&user.status),
                    {user_status_label(&user.status)}
                }
            }
            td { "data-label": t!("admin-users-blacklist-history"), class: "admin-users__history",
                if user.blacklist_history.is_empty() {
                    span { class: "admin-users__history-empty", {t!("admin-users-blacklist-never")} }
                } else {
                    details {
                        summary {
                            {user.blacklist_history.first().map(|entry| entry.public_reason.clone()).unwrap_or_default()}
                        }
                        ol {
                            for entry in user.blacklist_history.iter() {
                                li {
                                    span { {entry.public_reason.clone()} }
                                    time { datetime: entry.blacklisted_at.clone(), "{format_admin_time(&entry.blacklisted_at)}" }
                                }
                            }
                        }
                        if user.blacklist_count > user.blacklist_history.len() as i64 {
                            p { {t!("admin-users-history-more", count: user.blacklist_count - user.blacklist_history.len() as i64)} }
                        }
                    }
                }
            }
            td {
                "data-label": t!("admin-users-blacklist-count"),
                class: "public-auction-catalog__number",
                "{user.blacklist_count}"
            }
            td { "data-label": t!("admin-users-created-at"),
                time { datetime: user.created_at.clone(), "{format_admin_time(&user.created_at)}" }
            }
            td { "data-label": t!("admin-users-key-status"),
                span {
                    class: user_key_status_class(&user.key_status),
                    {user_key_status_label(&user.key_status)}
                }
            }
            td { "data-label": t!("admin-users-version"), "{user.state_version}" }
            td { "data-label": t!("admin-users-actions"), class: "admin-users__row-actions",
                input {
                    value: rename_value(),
                    aria_label: t!("admin-users-rename-input"),
                            oninput: move |event| {
                                rename_value.set(event.value());
                                command_state.set(None);
                            },
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| {
                        let user = user_for_rename.clone();
                        let next_username = rename_value();
                        let command_id = stable_command_id(
                            command_state,
                            format!("rename:{}:{next_username}", user.state_version),
                        );
                        run_user_action(
                            busy,
                            feedback,
                            reload_generation,
                            command_state,
                            on_generated_key,
                            user.username.clone(),
                            async move {
                            mutate_admin_user(command_id, &user, "rename", Some(next_username), None)
                                .await
                                .map(|_| None)
                            },
                        );
                    },
                    {t!("admin-users-action-touch-rename")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::GamePlayerId)),
                    {t!("admin-users-action-set-game-player-id")}
                }
                button {
                    r#type: "button",
                    disabled: busy() || user.key_status == "revoked",
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::RevokeKey)),
                    {t!("admin-users-action-revoke-key")}
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| dialog.set(Some(AdminUserDialogKind::ReissueKey)),
                    {t!("admin-users-action-reissue-key")}
                }
                if user.status == "blacklisted" {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::Unblacklist)),
                        {t!("admin-users-action-unblacklist")}
                    }
                } else {
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            dialog_reason.set(t!("admin-users-default-reason"));
                            dialog.set(Some(AdminUserDialogKind::Blacklist));
                        },
                        {t!("admin-users-action-blacklist")}
                    }
                }
                if let Some(message) = feedback() {
                    span { class: "admin-users__feedback", {message} }
                }
                if let Some(kind) = dialog() {
                    div { class: "admin-settlement__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-settlement__dialog admin-users-dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: "admin-users-dialog-title",
                            aria_busy: busy().to_string(),
                            tabindex: "-1",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    dialog.set(None);
                                }
                            },
                            h2 { id: "admin-users-dialog-title", {dialog_title.clone().unwrap_or_default()} }
                            if kind == AdminUserDialogKind::GamePlayerId {
                                form {
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() {
                                            return;
                                        }
                                        let user = user_for_game_dialog.clone();
                                        let value = game_player_id_value();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("set_game_player_id:{}:{value}", user.state_version),
                                        );
                                        let mut dialog = dialog;
                                        run_user_action(
                                            busy,
                                            feedback,
                                            reload_generation,
                                            command_state,
                                            on_generated_key,
                                            user.username.clone(),
                                            async move {
                                            let result = mutate_admin_user(
                                                command_id,
                                                &user,
                                                "set_game_player_id",
                                                None,
                                                Some(value),
                                            )
                                            .await;
                                            if result.is_ok() {
                                                dialog.set(None);
                                            }
                                            result.map(|_| None)
                                            },
                                        );
                                    },
                                    label {
                                        {t!("admin-users-game-player-id")}
                                        input {
                                            r#type: "text",
                                            autofocus: true,
                                            required: true,
                                            disabled: busy(),
                                            value: game_player_id_value(),
                                            oninput: move |event| game_player_id_value.set(event.value()),
                                        }
                                    }
                                    AdminUserDialogActions {
                                        busy,
                                        on_cancel: move |_| dialog.set(None),
                                    }
                                }
                            } else if kind == AdminUserDialogKind::Blacklist {
                                form {
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() || dialog_reason().trim().is_empty() {
                                            return;
                                        }
                                        let user = user_for_blacklist.clone();
                                        let reason = dialog_reason();
                                        let command_id = stable_command_id(
                                            command_state,
                                            format!("blacklist:{}:{reason}", user.state_version),
                                        );
                                        let mut dialog = dialog;
                                        run_user_action(
                                            busy,
                                            feedback,
                                            reload_generation,
                                            command_state,
                                            on_generated_key,
                                            user.username.clone(),
                                            async move {
                                            let result = blacklist_admin_user(command_id, &user, reason).await;
                                            if result.is_ok() {
                                                dialog.set(None);
                                            }
                                            result.map(|_| None)
                                            },
                                        );
                                    },
                                    label {
                                        {t!("admin-users-blacklist-reason")}
                                        textarea {
                                            autofocus: true,
                                            required: true,
                                            disabled: busy(),
                                            maxlength: "512",
                                            value: dialog_reason(),
                                            oninput: move |event| dialog_reason.set(event.value()),
                                        }
                                    }
                                    AdminUserDialogActions {
                                        busy,
                                        on_cancel: move |_| dialog.set(None),
                                    }
                                }
                            } else {
                                p { {t!("admin-users-confirm-description")} }
                                div { class: "admin-panel__actions",
                                    button {
                                        r#type: "button",
                                        class: "admin-btn-secondary",
                                        disabled: busy(),
                                        autofocus: true,
                                        onclick: move |_| dialog.set(None),
                                        {t!("admin-users-action-cancel")}
                                    }
                                    button {
                                        r#type: "button",
                                        class: if kind == AdminUserDialogKind::Unblacklist || kind == AdminUserDialogKind::ReissueKey {
                                            "admin-btn-primary"
                                        } else {
                                            "admin-btn-danger"
                                        },
                                        disabled: busy(),
                                        onclick: move |_| {
                                            let user = match kind {
                                                AdminUserDialogKind::RevokeKey => user_for_revoke.clone(),
                                                AdminUserDialogKind::ReissueKey => user_for_reissue.clone(),
                                                AdminUserDialogKind::Unblacklist => user_for_action.clone(),
                                                _ => return,
                                            };
                                            let action = match kind {
                                                AdminUserDialogKind::RevokeKey => "revoke_key",
                                                AdminUserDialogKind::ReissueKey => "reissue_key",
                                                AdminUserDialogKind::Unblacklist => "unblacklist",
                                                _ => return,
                                            };
                                            let command_id = stable_command_id(
                                                command_state,
                                                format!("{action}:{}", user.state_version),
                                            );
                                            let mut dialog = dialog;
                                            run_user_action(
                                                busy,
                                                feedback,
                                                reload_generation,
                                                command_state,
                                                on_generated_key,
                                                user.username.clone(),
                                                async move {
                                                let result = match kind {
                                                    AdminUserDialogKind::RevokeKey => {
                                                        mutate_admin_user(command_id, &user, "revoke_key", None, None)
                                                            .await
                                                            .map(|result| result.generated_key)
                                                    }
                                                    AdminUserDialogKind::ReissueKey => {
                                                        mutate_admin_user(command_id, &user, "reissue_key", None, None)
                                                            .await
                                                            .map(|result| result.generated_key)
                                                    }
                                                    AdminUserDialogKind::Unblacklist => {
                                                        unblacklist_admin_user(command_id, &user).await.map(|_| None)
                                                    }
                                                    _ => Ok(None),
                                                };
                                                if result.is_ok() {
                                                    dialog.set(None);
                                                }
                                                result
                                                },
                                            );
                                        },
                                        if busy() { {t!("admin-action-working")} } else { {t!("admin-users-action-confirm")} }
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
