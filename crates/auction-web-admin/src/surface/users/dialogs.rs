//! Admin 用户状态确认对话框与通用动作辅助。

use auction_web_shared::t;
use dioxus::prelude::*;
use uuid::Uuid;

use super::SourceKeyDelivery;
use super::create::AdminUserGeneratedKey;
use crate::api::{AdminUserStatusFilter, AdminWriteFailure};
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
    F: std::future::Future<Output = Result<Option<String>, AdminWriteFailure>> + 'static,
{
    let key_delivery = try_consume_context::<SourceKeyDelivery>();
    owner.submit_with_receipt(
        entity,
        action,
        operation,
        move |key, source_alive| {
            if source_alive
                && key.is_some()
                && let Some(delivery) = key_delivery
            {
                delivery.record();
            }
            AdminUserGeneratedKey::delivery(
                key,
                source_alive && try_set_if_alive(feedback, Some(AdminActionFeedback::Success)),
            )
        },
        move |code| {
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use dioxus::prelude::{Element, dioxus_core, dioxus_elements, dioxus_signals, rsx, use_signal};
    use gloo_net::http::Method;

    use super::run_user_action;
    use crate::api::{
        AdminApiFailure, AdminUser, blacklist_admin_user, mutate_admin_user, unblacklist_admin_user,
    };
    use crate::surface::auctions::draft_page_tests::{button, click_button, mount_writer};
    use crate::surface::pending::{PendingCommandPanel, use_pending_commands};

    const USER_ID: &str = "00000000-0000-4000-8000-000000000002";
    const ENTITY: &str = "user:00000000-0000-4000-8000-000000000002";
    const BLACKLIST: &str = "/api/v1/admin/users/00000000-0000-4000-8000-000000000002/blacklist";
    const UNKNOWN: &str =
        r#"{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}"#;
    const CONFLICT: &str = r#"{"code":"state_conflict","message":"用户版本已变化","data":{"current_state_version":4},"request_id":null}"#;

    #[test]
    fn user_actions_keep_or_clear_the_prior_unknown_with_the_same_command_id() {
        for (method, path) in [
            (Method::PATCH, "/api/v1/admin/users"),
            (Method::PUT, BLACKLIST),
            (Method::DELETE, BLACKLIST),
        ] {
            for status in [409, 422] {
                let attempts = Rc::new(RefCell::new(Vec::new()));
                let (mut page, _) = mount_writer({
                    let attempts = attempts.clone();
                    let method = method.clone();
                    move || -> Element {
                        let attempts = attempts.clone();
                        let method = method.clone();
                        let reload = use_signal(|| 0);
                        let feedback = use_signal(|| None);
                        let owner = use_pending_commands(reload, |_| true);
                        rsx! {
                            button {
                                disabled: owner.contains(ENTITY),
                                onclick: move |_| {
                                    let attempts = attempts.clone();
                                    let method = method.clone();
                                    assert!(run_user_action(
                                        owner, ENTITY.to_owned(), "用户写入".to_owned(), feedback, reload,
                                        move |command_id| {
                                            let method = method.clone();
                                            let first = attempts.borrow().is_empty();
                                            attempts.borrow_mut().push(command_id);
                                            async move {
                                                AdminApiFailure::stub_write_response(
                                                    path, method.clone(),
                                                    if first { 503 } else { status },
                                                    if first { UNKNOWN } else { CONFLICT },
                                                );
                                                let user = AdminUser {
                                                    id: USER_ID.to_owned(),
                                                    username: "before".to_owned(),
                                                    forum_account_id: None,
                                                    status: if method == Method::DELETE { "blacklisted" } else { "active" }.to_owned(),
                                                    key_status: "active".to_owned(),
                                                    state_version: 3,
                                                    created_at: "2026-11-14T00:00:00Z".to_owned(),
                                                    updated_at: "2026-11-14T00:00:00Z".to_owned(),
                                                    blacklist_count: 0,
                                                    blacklist_history: Vec::new(),
                                                };
                                                match method {
                                                    Method::PATCH => mutate_admin_user(command_id, &user, "rename", Some("after".into()), None)
                                                        .await.map(|receipt| receipt.generated_key),
                                                    Method::PUT => blacklist_admin_user(command_id, &user, "reason".into()).await.map(|_| None),
                                                    Method::DELETE => unblacklist_admin_user(command_id, &user).await.map(|_| None),
                                                    _ => unreachable!("只测试用户修改及黑名单"),
                                                }
                                            }
                                        },
                                    ));
                                },
                                {"提交用户写入".to_owned()}
                            }
                            PendingCommandPanel { owner }
                        }
                    }
                });
                click_button(&mut page, "提交用户写入");
                assert!(button(&page, "提交用户写入").unwrap().1);
                click_button(&mut page, "原样重试");
                let attempts = attempts.borrow();
                assert_eq!(attempts.len(), 2);
                assert_eq!(attempts[0], attempts[1]);
                assert_eq!(button(&page, "原样重试").is_some(), status != 409);
                assert_eq!(button(&page, "提交用户写入").unwrap().1, status != 409);
            }
        }
    }
}
