//! Admin 用户状态确认对话框与通用动作辅助。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::AdminUserStatusFilter;
use crate::app::surface::AdminActionFeedback;

use super::create::AdminUserGeneratedKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdminUserDialogKind {
    ForumAccountId,
    ReissueKey,
    RevokeKey,
    Blacklist,
    Unblacklist,
}

impl AdminUserDialogKind {
    pub(super) fn title(self, username: &str) -> String {
        match self {
            Self::ForumAccountId => {
                t!("admin-users-forum-account-dialog-title", username: username)
            }
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
    confirm_label: String,
    confirm_class: String,
) -> Element {
    rsx! {
        div { class: "admin-shell__dialog-actions",
            button {
                r#type: "button",
                disabled: busy(),
                onclick: on_cancel,
                {t!("admin-users-action-cancel")}
            }
            button {
                r#type: "submit",
                class: confirm_class,
                disabled: busy(),
                if busy() { {t!("admin-action-working")} } else { {confirm_label} }
            }
        }
    }
}

pub(super) fn run_user_action<F>(
    mut busy: Signal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
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
                on_generated_key.call(AdminUserGeneratedKey { key });
                Some(AdminActionFeedback::Success)
            }
            Ok(None) => Some(AdminActionFeedback::Success),
            Err(code) => Some(AdminActionFeedback::Error(code)),
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
