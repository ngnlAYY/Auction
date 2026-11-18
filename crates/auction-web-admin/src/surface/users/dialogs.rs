//! Admin 用户状态确认对话框与通用动作辅助。

use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::create::AdminUserGeneratedKey;
use crate::api::{AdminApiFailure, AdminUserStatusFilter};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::{PendingCommands, set_if_alive, try_set_if_alive};

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
    blocked: bool,
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
                disabled: busy() || blocked,
                if busy() { {t!("admin-action-working")} } else { {confirm_label} }
            }
        }
    }
}

/// 一次性 Key 由会话保留，普通用户反馈仍交付给页面。
pub(super) fn run_user_action<F>(
    owner: PendingCommands,
    entity: String,
    action: String,
    feedback: Signal<Option<AdminActionFeedback>>,
    reload_generation: Signal<u64>,
    operation: impl Fn(Uuid) -> F + 'static,
) -> bool
where
    F: std::future::Future<Output = Result<Option<String>, String>> + 'static,
{
    owner.submit_with_receipt(
        entity,
        action,
        operation,
        move |key, source_alive| {
            AdminUserGeneratedKey::delivery(
                key,
                source_alive && try_set_if_alive(feedback, Some(AdminActionFeedback::Success)),
            )
        },
        move |code| {
            let code = AdminApiFailure::display_code(&code).to_owned();
            set_if_alive(feedback, Some(AdminActionFeedback::Error(code)));
            // 成功由 owner 刷新；拒绝或未知也保留原用户页的重读行为。
            let mut reload = reload_generation;
            if let Ok(mut generation) = reload.try_write() {
                *generation = generation.saturating_add(1);
            }
        },
    )
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
