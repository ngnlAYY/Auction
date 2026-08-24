//! Admin 用户状态确认对话框与通用动作辅助。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::AdminUserStatusFilter;

use super::create::AdminUserGeneratedKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdminUserDialogKind {
    GamePlayerId,
    ReissueKey,
    RevokeKey,
    Blacklist,
    Unblacklist,
}

impl AdminUserDialogKind {
    pub(super) fn title(self, username: &str) -> String {
        match self {
            Self::GamePlayerId => t!("admin-users-game-dialog-title", username: username),
            Self::ReissueKey => t!("admin-users-reissue-dialog-title", username: username),
            Self::RevokeKey => t!("admin-users-revoke-dialog-title", username: username),
            Self::Blacklist => t!("admin-users-blacklist-dialog-title", username: username),
            Self::Unblacklist => t!("admin-users-unblacklist-dialog-title", username: username),
        }
    }
}

#[component]
pub(super) fn AdminUserDialogActions(
    busy: Signal<bool>,
    on_cancel: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div { class: "admin-panel__actions",
            button {
                r#type: "button",
                class: "admin-btn-secondary",
                disabled: busy(),
                onclick: on_cancel,
                {t!("admin-users-action-cancel")}
            }
            button {
                r#type: "submit",
                class: "admin-btn-primary",
                disabled: busy(),
                if busy() { {t!("admin-action-working")} } else { {t!("admin-users-action-confirm")} }
            }
        }
    }
}

pub(super) fn run_user_action<F>(
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<String>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
    username: String,
    operation: F,
) where
    F: std::future::Future<Output = Result<Option<String>, String>> + 'static,
{
    busy.set(true);
    spawn(async move {
        let result = operation.await;
        let successful = result.is_ok();
        feedback.set(match result {
            Ok(Some(key)) => {
                on_generated_key.call(AdminUserGeneratedKey { username, key });
                Some(t!("admin-users-updated"))
            }
            Ok(None) => Some(t!("admin-users-updated")),
            Err(_) => Some(t!("admin-users-action-error")),
        });
        if successful {
            command_state.set(None);
        }
        reload_generation.set(reload_generation().saturating_add(1));
        busy.set(false);
    });
}

pub(super) fn stable_command_id(
    mut command_state: Signal<Option<(String, Uuid)>>,
    signature: String,
) -> Uuid {
    match command_state() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            command_state.set(Some((signature, command_id)));
            command_id
        }
    }
}

pub(super) fn status_filter_value(value: AdminUserStatusFilter) -> &'static str {
    match value {
        AdminUserStatusFilter::All => "all",
        AdminUserStatusFilter::Active => "active",
        AdminUserStatusFilter::Blacklisted => "blacklisted",
    }
}

pub(super) fn parse_status_filter(value: &str) -> AdminUserStatusFilter {
    match value {
        "active" => AdminUserStatusFilter::Active,
        "blacklisted" => AdminUserStatusFilter::Blacklisted,
        _ => AdminUserStatusFilter::All,
    }
}

pub(super) fn user_status_label(value: &str) -> String {
    match value {
        "active" => t!("admin-users-status-active"),
        "blacklisted" => t!("admin-users-status-blacklisted"),
        _ => value.to_owned(),
    }
}

pub(super) fn user_status_class(value: &str) -> &'static str {
    match value {
        "active" => "admin-users__status admin-users__status--active",
        "blacklisted" => "admin-users__status admin-users__status--blacklisted",
        _ => "admin-users__status",
    }
}

pub(super) fn user_key_status_label(value: &str) -> String {
    match value {
        "active" => t!("admin-users-key-active"),
        "revoked" => t!("admin-users-key-revoked"),
        _ => value.to_owned(),
    }
}

pub(super) fn user_key_status_class(value: &str) -> &'static str {
    match value {
        "active" => "admin-users__key-status admin-users__key-status--active",
        "revoked" => "admin-users__key-status admin-users__key-status--revoked",
        _ => "admin-users__key-status",
    }
}
