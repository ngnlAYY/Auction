//! Admin 用户行展示、黑名单历史提示和用户状态操作。

use std::sync::atomic::{AtomicUsize, Ordering};

use auction_web_shared::{Dialog, Time, TruncatedText, legacy_t as t};
use dioxus::prelude::*;

use crate::api::{AdminUser, blacklist_admin_user, mutate_admin_user, unblacklist_admin_user};
use crate::surface::AdminActionFeedback;
use crate::surface::pending::PendingCommands;

use super::create::AdminUserGeneratedKey;
use super::dialogs::{AdminUserDialogActions, AdminUserDialogKind, run_user_action};

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
    busy: Signal<bool>,
    feedback: Signal<Option<AdminActionFeedback>>,
    on_generated_key: EventHandler<AdminUserGeneratedKey>,
) -> Element {
    let mut forum_account_id_value =
        use_signal(|| user.forum_account_id.clone().unwrap_or_default());
    let mut dialog = use_signal(|| None::<AdminUserDialogKind>);
    let mut dialog_reason = use_signal(String::new);
    let owner = use_context::<PendingCommands>();
    let blocked = owner.contains(&format!("user:{}", user.id));
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
                "data-label": t!("admin-users-username"),
                h3 { class: "admin-visually-hidden", {user.username.clone()} }
                TruncatedText { value: user.username.clone() }
            }
            td {
                "data-label": t!("admin-users-forum-account-id"),
                TruncatedText {
                    value: user
                        .forum_account_id
                        .clone()
                        .unwrap_or_else(|| t!("admin-users-empty")),
                }
            }
            td { "data-label": t!("admin-users-status"),
                span { class: "admin-users-badge", {status_label} }
            }
            td {
                "data-label": t!("admin-users-blacklist-history"),
                class: "admin-users-history-cell",
                BlacklistHistoryCell { user: user_for_history }
            }
            td { "data-label": t!("admin-users-blacklist-count"), "{user.blacklist_count}" }
            td { "data-label": t!("admin-users-created-at"),
                Time { value: user.created_at.clone() }
            }
            td {
                "data-label": t!("admin-users-actions"),
                div { class: "admin-users-actions",
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| {
                            forum_account_id_value
                                .set(user.forum_account_id.clone().unwrap_or_default());
                            dialog.set(Some(AdminUserDialogKind::ForumAccountId));
                        },
                        {t!("admin-users-action-set-forum-account-id")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked,
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::ReissueKey)),
                        {t!("admin-users-action-reissue-key")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || blocked || user.key_status == "revoked",
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::RevokeKey)),
                        {t!("admin-users-action-revoke-key")}
                    }
                    if user.status == "blacklisted" {
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy() || blocked,
                            onclick: move |_| dialog.set(Some(AdminUserDialogKind::Unblacklist)),
                            {t!("admin-users-action-unblacklist")}
                        }
                    } else {
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: busy() || blocked,
                            onclick: move |_| {
                                dialog_reason.set(String::new());
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
                        title: dialog_title.clone().unwrap_or_default(),
                        close_label: t!("admin-users-action-cancel"),
                        busy: busy(),
                        on_close: move |_| dialog.set(None),
                        if kind == AdminUserDialogKind::ForumAccountId {
                                form {
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() {
                                            return;
                                        }
                                        let user = user_for_forum_dialog.clone();
                                        let value = forum_account_id_value();
                                        if run_user_action(
                                            owner, format!("user:{}", user.id), t!("admin-users-action-set-forum-account-id"),
                                            feedback, reload_generation, on_generated_key,
                                            move |command_id| {
                                                let user = user.clone();
                                                let value = value.clone();
                                                async move { mutate_admin_user(command_id, &user, "set_forum_account_id", None, Some(value)).await.map(|_| None) }
                                            },
                                        ) { dialog.set(None); }
                                    },
                                    label {
                                        r#for: format!("forum-account-{}", user.id),
                                        {t!("admin-users-forum-account-id")}
                                    }
                                    input {
                                        id: format!("forum-account-{}", user.id),
                                        r#type: "text",
                                        inputmode: "numeric",
                                        pattern: "[1-9][0-9]*",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: forum_account_id_value(),
                                        oninput: move |event| forum_account_id_value.set(event.value()),
                                    }
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
                                    onsubmit: move |event| {
                                        event.prevent_default();
                                        if busy() || dialog_reason().trim().is_empty() {
                                            return;
                                        }
                                        let user = user_for_blacklist.clone();
                                        let reason = dialog_reason();
                                        if run_user_action(
                                            owner, format!("user:{}", user.id), t!("admin-users-action-blacklist"),
                                            feedback, reload_generation, on_generated_key,
                                            move |command_id| {
                                                let user = user.clone();
                                                let reason = reason.clone();
                                                async move { blacklist_admin_user(command_id, &user, reason).await.map(|_| None) }
                                            },
                                        ) { dialog.set(None); }
                                    },
                                    p { {t!("admin-users-blacklist-warning")} }
                                    label {
                                        r#for: format!("reason-{}", user.id),
                                        {t!("admin-users-blacklist-reason")}
                                    }
                                    textarea {
                                        id: format!("reason-{}", user.id),
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        maxlength: "512",
                                        value: dialog_reason(),
                                        oninput: move |event| dialog_reason.set(event.value()),
                                    }
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
                                            if busy() { return; }
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
                                                owner, format!("user:{}", user.id), kind.title(&user.username),
                                                feedback, reload_generation, on_generated_key,
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
                    #[cfg(target_arch = "wasm32")]
                    {
                        let Some(element) = event.data().downcast::<web_sys::Element>().cloned() else {
                            return;
                        };
                        mounted_element.set(Some(element));
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = event;
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
                    Time { value: entry.blacklisted_at.clone() }
                }
            }
        }
        if user.blacklist_count > user.blacklist_history.len() as i64 {
            p { {t!("admin-users-blacklist-history-limited")} }
        }
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
    let (viewport_width, viewport_height) = web_sys::window()
        .map(|window| {
            (
                window.inner_width().ok().and_then(|value| value.as_f64()).unwrap_or(1024.0),
                window.inner_height().ok().and_then(|value| value.as_f64()).unwrap_or(768.0),
            )
        })
        .unwrap_or((1024.0, 768.0));
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
