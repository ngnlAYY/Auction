//! Admin 用户行展示、黑名单历史提示和用户状态操作。

use std::sync::atomic::{AtomicUsize, Ordering};

use auction_web_shared::{Dialog, Time, TruncatedText, t};
use dioxus::prelude::*;

use super::dialogs::{AdminUserDialogActions, AdminUserDialogKind, run_user_action};
use super::fields::{UserFieldError, UserFieldIssue, blacklist_reason_issue, forum_account_issue};
use super::user_command_entity;
use crate::api::{AdminUser, blacklist_admin_user, mutate_admin_user, unblacklist_admin_user};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::PendingCommands;
use crate::surface::validation::focus_field;

static NEXT_HISTORY_TOOLTIP_ID: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone, Copy, PartialEq)]
struct HistoryTooltipPosition {
    left: f64,
    top: f64,
    width: f64,
    maximum_height: f64,
    placement: &'static str,
}

#[component]
pub(super) fn AdminUserRow(
    user: AdminUser,
    reload_generation: Signal<u64>,
    loaded_generation: u64,
    busy: Signal<bool>,
    feedback: Signal<Option<AdminActionFeedback>>,
) -> Element {
    let mut forum_account_id_value =
        use_signal(|| user.forum_account_id.clone().unwrap_or_default());
    let mut dialog = use_signal(|| None::<AdminUserDialogKind>);
    let mut dialog_reason = use_signal(String::new);
    let mut field_issue = use_signal(|| None::<UserFieldIssue>);
    let owner = use_context::<PendingCommands>();
    // 原命令了结后旧行可显示，获取当前用户版本前不能打开或提交会写入的对话框。
    let read_is_current = move || reload_generation() == loaded_generation;
    let status_recognized = matches!(user.status.as_str(), "active" | "blacklisted");
    let blocked =
        !read_is_current() || !status_recognized || owner.contains(&user_command_entity(&user.id));
    let user_for_forum_dialog = user.clone();
    let user_for_revoke = user.clone();
    let user_for_reissue = user.clone();
    let user_for_blacklist = user.clone();
    let user_for_action = user.clone();
    let user_for_history = user.clone();
    let status_label = match user.status.as_str() {
        "active" => t!("admin-users-status-active"),
        "blacklisted" => t!("admin-users-status-blacklisted"),
        _ => t!("admin-error-invalid-response"),
    };
    let dialog_title = dialog().map(|kind| kind.title(&user.username));

    rsx! {
        tr {
            td {
                TruncatedText { value: user.username.clone() }
            }
            td {
                TruncatedText {
                    value: user
                        .forum_account_id
                        .clone()
                        .unwrap_or_else(|| t!("admin-users-empty")),
                }
            }
            td {
                span { class: "admin-users-badge", {status_label} }
            }
            td {
                class: "admin-users-history-cell",
                BlacklistHistoryCell { user: user_for_history }
            }
            td { "{user.blacklist_count}" }
            td {
                Time { value: user.created_at.clone() }
            }
            td {
                div { class: "admin-users-actions",
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| {
                            if !read_is_current() { return; }
                            forum_account_id_value
                                .set(user.forum_account_id.clone().unwrap_or_default());
                            field_issue.set(None);
                            dialog.set(Some(AdminUserDialogKind::ForumAccountId));
                        },
                        {t!("admin-users-action-set-forum-account-id")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| {
                            if read_is_current() { dialog.set(Some(AdminUserDialogKind::ReissueKey)); }
                        },
                        {t!("admin-users-action-reissue-key")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || user.key_status == "revoked",
                        onclick: move |_| {
                            if read_is_current() { dialog.set(Some(AdminUserDialogKind::RevokeKey)); }
                        },
                        {t!("admin-users-action-revoke-key")}
                    }
                    if user.status == "blacklisted" {
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy() || blocked,
                            onclick: move |_| {
                                if read_is_current() { dialog.set(Some(AdminUserDialogKind::Unblacklist)); }
                            },
                            {t!("admin-users-action-unblacklist")}
                        }
                    } else {
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: busy() || blocked,
                            onclick: move |_| {
                                if !read_is_current() { return; }
                                dialog_reason.set(String::new());
                                field_issue.set(None);
                                dialog.set(Some(AdminUserDialogKind::Blacklist));
                            },
                            {t!("admin-users-action-blacklist")}
                        }
                    }
                }
            }
        }
        if let Some(kind) = dialog() {
            tr { class: "admin-users-dialog-row",
                td { colspan: "7",
                    Dialog {
                        title: dialog_title.unwrap_or_default(),
                        close_label: t!("admin-users-action-cancel"),
                        busy: busy(),
                        on_close: move |_| dialog.set(None),
                        if kind == AdminUserDialogKind::ForumAccountId {
                                form {
                                    method: "post",
                                    action: "/api/v1/admin/users",
                                    aria_label: t!("admin-users-action-set-forum-account-id"),
                                    novalidate: true,
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() || !read_is_current() {
                                            return;
                                        }
                                        let user = user_for_forum_dialog.clone();
                                        let value = forum_account_id_value();
                                        let issue = forum_account_issue(&value, true);
                                        field_issue.set(issue);
                                        if issue.is_some() {
                                            focus_field("admin-user-edit-forum-account");
                                            return;
                                        }
                                        if run_user_action(
                                            owner, user_command_entity(&user.id), t!("admin-users-action-set-forum-account-id"),
                                            feedback, reload_generation,
                                            move |command_id| {
                                                let user = user.clone();
                                                let value = value.clone();
                                                async move { mutate_admin_user(command_id, &user, "set_forum_account_id", None, Some(value)).await.map(|_| None) }
                                            },
                                        ) { dialog.set(None); }
                                    },
                                    label {
                                        r#for: "admin-user-edit-forum-account",
                                        {t!("admin-users-forum-account-id")}
                                        span { aria_hidden: "true", " *" }
                                    }
                                    input {
                                        id: "admin-user-edit-forum-account",
                                        name: "forum_account_id",
                                        r#type: "text",
                                        inputmode: "numeric",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        aria_invalid: field_issue().is_some().to_string(),
                                        aria_describedby: field_issue().is_some().then_some("admin-user-edit-forum-account-error"),
                                        value: forum_account_id_value(),
                                        oninput: move |event| {
                                            let value = event.value();
                                            if field_issue.peek().is_some() {
                                                field_issue.set(forum_account_issue(&value, true));
                                            }
                                            forum_account_id_value.set(value);
                                        },
                                    }
                                    UserFieldError { id: "admin-user-edit-forum-account-error", issue: field_issue() }
                                    AdminUserDialogActions {
                                        busy,
                                        blocked,
                                        on_cancel: move |_| dialog.set(None),
                                        confirm_label: t!("admin-users-save-forum-account-id"),
                                        confirm_class: "admin-btn-primary".to_owned(),
                                    }
                                }
                            } else if kind == AdminUserDialogKind::Blacklist {
                                form {
                                    method: "post",
                                    action: format!("/api/v1/admin/users/{}/blacklist", user.id),
                                    aria_label: t!("admin-users-action-blacklist"),
                                    novalidate: true,
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() || !read_is_current() {
                                            return;
                                        }
                                        let user = user_for_blacklist.clone();
                                        let reason = dialog_reason();
                                        let issue = blacklist_reason_issue(&reason);
                                        field_issue.set(issue);
                                        if issue.is_some() {
                                            focus_field("admin-user-blacklist-reason");
                                            return;
                                        }
                                        if run_user_action(
                                            owner, user_command_entity(&user.id), t!("admin-users-action-blacklist"),
                                            feedback, reload_generation,
                                            move |command_id| {
                                                let user = user.clone();
                                                let reason = reason.clone();
                                                async move { blacklist_admin_user(command_id, &user, reason).await.map(|_| None) }
                                            },
                                        ) { dialog.set(None); }
                                    },
                                    p { {t!("admin-users-blacklist-warning")} }
                                    label {
                                        r#for: "admin-user-blacklist-reason",
                                        {t!("admin-users-blacklist-reason")}
                                    }
                                    textarea {
                                        id: "admin-user-blacklist-reason",
                                        name: "public_reason",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        aria_invalid: field_issue().is_some().to_string(),
                                        aria_describedby: field_issue().is_some().then_some("admin-user-blacklist-reason-error"),
                                        value: dialog_reason(),
                                        oninput: move |event| {
                                            let value = event.value();
                                            if field_issue.peek().is_some() {
                                                field_issue.set(blacklist_reason_issue(&value));
                                            }
                                            dialog_reason.set(value);
                                        },
                                    }
                                    UserFieldError { id: "admin-user-blacklist-reason-error", issue: field_issue() }
                                    AdminUserDialogActions {
                                        busy,
                                        blocked,
                                        on_cancel: move |_| dialog.set(None),
                                        confirm_label: t!("admin-users-action-blacklist"),
                                        confirm_class: "admin-btn-danger".to_owned(),
                                    }
                                }
                            } else {
                                if kind == AdminUserDialogKind::ReissueKey {
                                    p { {t!("admin-users-reissue-warning")} }
                                } else if kind == AdminUserDialogKind::RevokeKey {
                                    p { {t!("admin-users-revoke-warning")} }
                                }
                                div { class: "admin-shell__dialog-actions",
                                    button {
                                        r#type: "button",
                                        disabled: busy(),
                                        autofocus: true,
                                        onclick: move |_| dialog.set(None),
                                        {t!("admin-users-action-cancel")}
                                    }
                                    button {
                                        r#type: "button",
                                        class: if kind == AdminUserDialogKind::RevokeKey {
                                            "admin-btn-danger"
                                        } else {
                                            "admin-btn-primary"
                                        },
                                        disabled: busy() || blocked,
                                        onclick: move |_| {
                                            if busy() || !read_is_current() { return; }
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
                                            if run_user_action(
                                                owner, user_command_entity(&user.id), kind.title(&user.username),
                                                feedback, reload_generation,
                                                move |command_id| {
                                                    let user = user.clone();
                                                    async move {
                                                        if kind == AdminUserDialogKind::Unblacklist {
                                                            unblacklist_admin_user(command_id, &user).await.map(|_| None)
                                                        } else {
                                                            mutate_admin_user(command_id, &user, action, None, None).await.map(|result| result.generated_key)
                                                        }
                                                    }
                                                },
                                            ) { dialog.set(None); }
                                        },
                                        if busy() {
                                            {t!("admin-action-working")}
                                        } else {
                                            {t!("admin-users-action-confirm")}
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

#[component]
fn BlacklistHistoryCell(user: AdminUser) -> Element {
    let mut focused = use_signal(|| false);
    let mut history_open = use_signal(|| false);
    let mut tooltip_position = use_signal(|| None::<HistoryTooltipPosition>);
    use_history_tooltip_geometry_guard(tooltip_position);
    let tooltip_id = use_signal(|| {
        format!(
            "admin-users-history-tooltip-{}",
            NEXT_HISTORY_TOOLTIP_ID.fetch_add(1, Ordering::Relaxed),
        )
    });
    #[cfg(target_arch = "wasm32")]
    let mut mounted_element = use_signal(|| None::<web_sys::Element>);

    let latest = user.blacklist_history.first().cloned();
    let tooltip = tooltip_position();
    let tooltip_id = tooltip_id();
    let described_by = tooltip.map(|_| tooltip_id.clone());

    rsx! {
        if let Some(latest) = latest {
            button {
                r#type: "button",
                class: "admin-users-history-trigger",
                aria_describedby: described_by,
                aria_haspopup: "dialog",
                aria_label: t!(
                    "admin-users-blacklist-history-label",
                    count: user.blacklist_count
                ),
                onmounted: move |event| {
                    cfg_select! {
                        target_arch = "wasm32" => {
                            let Some(element) =
                                event.data().downcast::<web_sys::Element>().cloned()
                            else {
                                return;
                            };
                            mounted_element.set(Some(element));
                        }
                        _ => { let _ = event; }
                    }
                },
                onmouseenter: move |_| {
                    #[cfg(target_arch = "wasm32")]
                    show_history_tooltip(mounted_element(), tooltip_position);
                },
                onmouseleave: move |_| {
                    if !focused() {
                        tooltip_position.set(None);
                    }
                },
                onfocus: move |_| {
                    focused.set(true);
                    #[cfg(target_arch = "wasm32")]
                    show_history_tooltip(mounted_element(), tooltip_position);
                },
                onblur: move |_| {
                    focused.set(false);
                    tooltip_position.set(None);
                },
                // 触控和键盘激活都进入可滚动详情，长历史不依赖只读的悬浮提示。
                onclick: move |_| {
                    tooltip_position.set(None);
                    history_open.set(true);
                },
                {latest.public_reason}
            }
            if let Some(position) = tooltip {
                aside {
                    id: tooltip_id,
                    class: "admin-users-history-tooltip",
                    role: "tooltip",
                    "data-placement": position.placement,
                    style: "--admin-users-history-tooltip-left: {position.left}px; --admin-users-history-tooltip-top: {position.top}px; --admin-users-history-tooltip-width: {position.width}px; --admin-users-history-tooltip-max-height: {position.maximum_height}px;",
                    strong {
                        {t!(
                            "admin-users-blacklist-history-title",
                            count: user.blacklist_count
                        )}
                    }
                    BlacklistHistoryEntries { user: user.clone() }
                }
            }
            if history_open() {
                Dialog {
                    title: t!("admin-users-blacklist-history-title", count: user.blacklist_count),
                    close_label: t!("admin-users-blacklist-history-close"),
                    on_close: move |_| history_open.set(false),
                    div { class: "admin-users-history-details",
                        BlacklistHistoryEntries { user: user.clone() }
                    }
                }
            }
        } else {
            span { class: "admin-users-history-empty", {t!("admin-users-blacklist-never")} }
        }
    }
}

/// 悬浮摘要与触控详情使用同一份已加载历史，不发起额外用户查询。
#[component]
fn BlacklistHistoryEntries(user: AdminUser) -> Element {
    rsx! {
        ol {
            for entry in user.blacklist_history.iter().cloned() {
                li {
                    span { {entry.public_reason} }
                    Time { value: entry.blacklisted_at }
                }
            }
        }
        if user.blacklist_count > user.blacklist_history.len() as i64 {
            p { {t!("admin-users-blacklist-history-limited")} }
        }
    }
}

fn use_history_tooltip_geometry_guard(tooltip_position: Signal<Option<HistoryTooltipPosition>>) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::{cell::RefCell, rc::Rc};
        use wasm_bindgen::{JsCast, closure::Closure};

        let guard = use_hook(|| Rc::new(RefCell::new(None::<HistoryTooltipGeometryGuard>)));
        let cleanup = Rc::clone(&guard);
        use_drop(move || {
            cleanup.borrow_mut().take();
        });
        use_effect(move || {
            if tooltip_position.read().is_none() {
                guard.borrow_mut().take();
                return;
            }
            if guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else { return };
            let mut close_position = tooltip_position;
            let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
                close_position.set(None);
            });
            if window
                .add_event_listener_with_callback_and_bool(
                    "scroll",
                    callback.as_ref().unchecked_ref(),
                    true,
                )
                .is_err()
            {
                return;
            }
            if window
                .add_event_listener_with_callback("resize", callback.as_ref().unchecked_ref())
                .is_err()
            {
                let _ = window.remove_event_listener_with_callback_and_bool(
                    "scroll",
                    callback.as_ref().unchecked_ref(),
                    true,
                );
                return;
            }
            guard.replace(Some(HistoryTooltipGeometryGuard { window, callback }));
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = tooltip_position;
}

#[cfg(target_arch = "wasm32")]
struct HistoryTooltipGeometryGuard {
    window: web_sys::Window,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for HistoryTooltipGeometryGuard {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;

        let _ = self.window.remove_event_listener_with_callback_and_bool(
            "scroll",
            self.callback.as_ref().unchecked_ref(),
            true,
        );
        let _ = self
            .window
            .remove_event_listener_with_callback("resize", self.callback.as_ref().unchecked_ref());
    }
}

#[cfg(target_arch = "wasm32")]
fn show_history_tooltip(
    element: Option<web_sys::Element>,
    mut tooltip_position: Signal<Option<HistoryTooltipPosition>>,
) {
    let Some(element) = element else {
        return;
    };
    let rectangle = element.get_bounding_client_rect();
    let (viewport_width, viewport_height) = web_sys::window().map_or((1024.0, 768.0), |window| {
        (
            window.inner_width().ok().and_then(|value| value.as_f64()).unwrap_or(1024.0),
            window.inner_height().ok().and_then(|value| value.as_f64()).unwrap_or(768.0),
        )
    });
    let width = 360.0_f64.min((viewport_width - 16.0).max(1.0));
    let gap = 8.0;
    let margin = 8.0;
    let available_above = (rectangle.top() - gap - margin).max(1.0);
    let available_below = (viewport_height - rectangle.bottom() - gap - margin).max(1.0);
    let above = available_above > available_below;
    let maximum_left = (viewport_width - width - margin).max(margin);
    tooltip_position.set(Some(HistoryTooltipPosition {
        left: rectangle.left().max(margin).min(maximum_left),
        top: if above { rectangle.top() - gap } else { rectangle.bottom() + gap },
        width,
        maximum_height: if above { available_above } else { available_below },
        placement: if above { "above" } else { "below" },
    }));
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use dioxus::core::Mutation;
    use dioxus::prelude::*;

    use super::AdminUserRow;
    use crate::api::{AdminApiFailure, AdminUser};
    use crate::surface::auctions::editor::tests::{
        button, click_button, has_live_text, init_i18n, mount_writer,
    };
    use crate::surface::pending::{use_pending_command_gate, use_pending_commands_with_gate};
    use crate::surface::realtime::text_attributes;
    use crate::surface::users::{SourceGeneratedKey, SourceKeyDelivery};

    #[test]
    fn reissue_key_success_is_displayed_once_and_acknowledged() {
        const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        const USER_ID: &str = "00000000-0000-4000-8000-000000000026";
        const SUCCESS: &str = r#"{
            "code": "ok",
            "message": "已重签",
            "data": {
                "user": {
                    "id": "00000000-0000-4000-8000-000000000026",
                    "username": "ExamplePlayer",
                    "forum_account_id": null,
                    "status": "active",
                    "key_status": "active",
                    "state_version": 4,
                    "created_at": "2025-11-15T09:46:40.110000Z",
                    "updated_at": "2026-11-14T00:00:00Z"
                },
                "generated_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            }
        }"#;
        let attempts = Rc::new(Cell::new(0));
        let (mut page, _) = mount_writer({
            let attempts = attempts.clone();
            move || {
                init_i18n();
                let reload = use_signal(|| 0);
                let feedback = use_signal(|| None);
                let busy = use_signal(|| false);
                let delivered_revision = use_signal(|| None);
                use_context_provider(|| SourceKeyDelivery(delivered_revision));
                let gate = use_pending_command_gate();
                let owner = use_pending_commands_with_gate(reload, Some(gate), |_| true);
                let attempts = attempts.clone();
                use_hook(move || {
                    AdminApiFailure::stub_user_mutation_success(move |command| {
                        assert_eq!(command.action, "reissue_key");
                        assert_eq!(command.user_id, USER_ID);
                        assert_eq!(command.expected_state_version, 3);
                        attempts.set(attempts.get() + 1);
                        SUCCESS
                    });
                });
                let user = AdminUser {
                    id: USER_ID.to_owned(),
                    username: "ExamplePlayer".to_owned(),
                    forum_account_id: None,
                    status: "active".to_owned(),
                    key_status: "active".to_owned(),
                    state_version: 3,
                    created_at: "2025-11-15T09:46:40.110000Z".to_owned(),
                    updated_at: "2025-11-15T09:46:40.110000Z".to_owned(),
                    blacklist_count: 0,
                    blacklist_history: Vec::new(),
                };
                rsx! {
                    table {
                        tbody {
                            AdminUserRow {
                                user,
                                reload_generation: reload,
                                loaded_generation: 0,
                                busy,
                                feedback,
                            }
                        }
                    }
                    if let Some((command_id, key)) = owner.generated_key() {
                        SourceGeneratedKey {
                            generated: key,
                            delivered_revision: delivered_revision(),
                            on_acknowledge: move |_| owner.acknowledge_key(command_id),
                        }
                    }
                }
            }
        });
        assert!(!has_live_text(&page, KEY));
        let opened = click_button(&mut page, "重新签发 Key");
        assert_eq!(attempts.get(), 0);
        assert!(!has_live_text(&page, KEY));
        assert!(has_live_text(&page, "为 ExamplePlayer 重新签发 Key"));
        // 确认按钮的文案分支由 Dioxus 渲染为片段，通过弹窗新挂载的按钮定位。
        let confirm = text_attributes(&opened, "class")
            .find_map(|(button, class)| (class == "admin-btn-primary").then_some(button))
            .expect("重签对话框已挂载确认按钮");
        let edits = page.dispatch("click", confirm, "");
        assert_eq!(
            edits
                .iter()
                .filter(|edit| {
                    matches!(
                        edit,
                        Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                            if value == KEY
                    )
                })
                .count(),
            1,
            "真实用户行成功后应显示一次新 Key"
        );
        assert!(has_live_text(&page, KEY));
        click_button(&mut page, "关闭");
        page.rerender();
        assert!(!has_live_text(&page, KEY));
        assert!(button(&page, "关闭").is_none());
        assert_eq!(attempts.get(), 1);
    }
}
