//! Admin 用户状态确认对话框与通用动作辅助。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::AdminUserStatusFilter;
use crate::app::surface::{AdminActionFeedback, run_admin_owned_action};

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
    busy: Signal<bool>,
    mut feedback: Signal<Option<AdminActionFeedback>>,
    mut reload_generation: Signal<u64>,
    mut command_state: Signal<Option<(String, Uuid)>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
    operation: F,
) where
    F: std::future::Future<Output = Result<Option<String>, String>> + 'static,
{
    run_admin_owned_action(
        busy,
        async move {
            let result = operation.await;
            // 旧版无论成败都刷新一次列表投影。
            reload_generation.set(reload_generation().saturating_add(1));
            result
        },
        move |key| {
            if let Some(key) = key {
                on_generated_key.call(AdminUserGeneratedKey { key });
            }
            feedback.set(Some(AdminActionFeedback::Success));
            // 用户行可能已因筛选/分页卸载；页面结果仍需交付，旧行命令状态仅在存活时清理。
            if let Ok(mut command_state) = command_state.try_write() {
                *command_state = None;
            }
        },
        move |code| feedback.set(Some(AdminActionFeedback::Error(code))),
    );
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
