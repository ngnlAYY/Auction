//! Admin 用户行展示、黑名单历史提示和用户状态操作。

use std::sync::atomic::{AtomicUsize, Ordering};

use auction_web_shared::{TruncatedText, legacy_t as t};
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use crate::api::{AdminUser, blacklist_admin_user, mutate_admin_user, unblacklist_admin_user};
use crate::app::surface::AdminActionFeedback;

use super::super::format_admin_time;
use super::create::AdminUserGeneratedKey;
use super::dialogs::{
    AdminUserDialogActions, AdminUserDialogKind, run_user_action, stable_command_id,
};

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
    let mut game_player_id_value = use_signal(|| user.game_player_id.clone().unwrap_or_default());
    let mut dialog = use_signal(|| None::<AdminUserDialogKind>);
    let mut dialog_reason = use_signal(String::new);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let user_for_game_dialog = user.clone();
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
    let dialog_title_id = format!("admin-user-dialog-title-{}", user.id);

    rsx! {
        tr {
            td {
                "data-label": t!("admin-users-username"),
                h3 { class: "admin-visually-hidden", {user.username.clone()} }
                TruncatedText { value: user.username.clone() }
            }
            td {
                "data-label": t!("admin-users-game-player-id"),
                TruncatedText {
                    value: user
                        .game_player_id
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
                time { datetime: user.created_at.clone(), "{format_admin_time(&user.created_at)}" }
            }
            td {
                "data-label": t!("admin-users-actions"),
                div { class: "admin-users-actions",
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| {
                            game_player_id_value
                                .set(user.game_player_id.clone().unwrap_or_default());
                            dialog.set(Some(AdminUserDialogKind::GamePlayerId));
                        },
                        {t!("admin-users-action-set-game-player-id")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy(),
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::ReissueKey)),
                        {t!("admin-users-action-reissue-key")}
                    }
                    button {
                        r#type: "button",
                        disabled: busy() || user.key_status == "revoked",
                        onclick: move |_| dialog.set(Some(AdminUserDialogKind::RevokeKey)),
                        {t!("admin-users-action-revoke-key")}
                    }
                    if user.status == "blacklisted" {
                        button {
                            class: "admin-btn-secondary",
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| dialog.set(Some(AdminUserDialogKind::Unblacklist)),
                            {t!("admin-users-action-unblacklist")}
                        }
                    } else {
                        button {
                            class: "admin-btn-danger",
                            r#type: "button",
                            disabled: busy(),
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
                    div { class: "admin-shell__dialog-backdrop", role: "presentation",
                        section {
                            class: "admin-shell__dialog",
                            role: "dialog",
                            aria_modal: "true",
                            aria_labelledby: dialog_title_id.clone(),
                            onkeydown: move |event| {
                                if event.key() == Key::Escape && !busy() {
                                    event.stop_propagation();
                                    dialog.set(None);
                                }
                            },
                            h2 { id: dialog_title_id.clone(), {dialog_title.clone().unwrap_or_default()} }
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
                                        r#for: format!("game-{}", user.id),
                                        {t!("admin-users-game-player-id")}
                                    }
                                    input {
                                        id: format!("game-{}", user.id),
                                        r#type: "text",
                                        autofocus: true,
                                        required: true,
                                        disabled: busy(),
                                        value: game_player_id_value(),
                                        oninput: move |event| game_player_id_value.set(event.value()),
                                    }
                                    AdminUserDialogActions {
                                        busy,
                                        on_cancel: move |_| dialog.set(None),
                                        confirm_label: t!("admin-users-save-game-id"),
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
                                            async move {
                                                let result = blacklist_admin_user(
                                                    command_id,
                                                    &user,
                                                    reason,
                                                )
                                                .await;
                                                if result.is_ok() {
                                                    dialog.set(None);
                                                }
                                                result.map(|_| None)
                                            },
                                        );
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
                                                async move {
                                                    let result = match kind {
                                                        AdminUserDialogKind::RevokeKey => mutate_admin_user(
                                                            command_id,
                                                            &user,
                                                            "revoke_key",
                                                            None,
                                                            None,
                                                        )
                                                        .await
                                                        .map(|result| result.generated_key),
                                                        AdminUserDialogKind::ReissueKey => mutate_admin_user(
                                                            command_id,
                                                            &user,
                                                            "reissue_key",
                                                            None,
                                                            None,
                                                        )
                                                        .await
                                                        .map(|result| result.generated_key),
                                                        AdminUserDialogKind::Unblacklist => {
                                                            unblacklist_admin_user(command_id, &user)
                                                                .await
                                                                .map(|_| None)
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
}

#[component]
fn BlacklistHistoryCell(user: AdminUser) -> Element {
    let mut focused = use_signal(|| false);
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
                    ol {
                        for entry in user.blacklist_history.iter().cloned() {
                            li {
                                span { {entry.public_reason} }
                                time {
                                    datetime: entry.blacklisted_at.clone(),
                                    "{format_admin_time(&entry.blacklisted_at)}"
                                }
                            }
                        }
                    }
                    if user.blacklist_count > user.blacklist_history.len() as i64 {
                        p { {t!("admin-users-blacklist-history-limited")} }
                    }
                }
            }
        } else {
            span { class: "admin-users-history-empty", {t!("admin-users-blacklist-never")} }
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
