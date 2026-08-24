//! Admin 拍卖生命周期动作按钮与确认对话框。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    AdminAuctionCapabilities, AdminAuctionLifecycleAction, execute_admin_auction_lifecycle,
};

use super::model::{
    lifecycle_action_button_class, lifecycle_action_confirm_note, lifecycle_action_confirm_title,
    lifecycle_action_label, lifecycle_action_submit_label, lifecycle_actions,
};

#[component]
pub(super) fn AdminAuctionActions(
    auction_id: String,
    auction_sequence: i64,
    status: String,
    expected_state_version: i64,
    capabilities: AdminAuctionCapabilities,
    scheduled_open_at: String,
    current_time_millis: i64,
    notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
) -> Element {
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<bool>);
    let pending_action = use_signal(|| None::<AdminAuctionLifecycleAction>);
    let cancel_reason = use_signal(String::new);
    let command_state = use_signal(|| None::<(String, Uuid)>);
    let actions =
        lifecycle_actions(&status, &capabilities, &scheduled_open_at, current_time_millis);

    rsx! {
        div { class: "admin-auctions__actions",
            Link {
                class: "admin-auctions__open-link",
                to: super::super::super::Route::AuctionLots { auction_id: auction_id.clone() },
                {t!("admin-auctions-open")}
            }
            for action in actions {
                AdminAuctionActionButton {
                    action,
                    busy,
                    notice,
                    pending_action,
                }
            }
        }
        if let Some(action) = pending_action() {
            AdminAuctionActionDialog {
                auction_id,
                auction_sequence,
                expected_state_version,
                action,
                busy,
                feedback,
                notice,
                reload_generation,
                pending_action,
                cancel_reason,
                command_state,
            }
        }
        if feedback() == Some(false) {
            span {
                class: "admin-panel__note action-feedback action-feedback--error",
                role: "alert",
                {t!("admin-auctions-action-error")}
            }
        }
    }
}

#[component]
fn AdminAuctionActionButton(
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionLifecycleAction>>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
) -> Element {
    rsx! {
        button {
            class: lifecycle_action_button_class(action),
            r#type: "button",
            disabled: busy(),
            onclick: move |_| {
                if busy() {
                    return;
                }
                notice.set(None);
                pending_action.set(Some(action));
            },
            if busy() { {t!("admin-action-working")} } else { {lifecycle_action_label(action)} }
        }
    }
}

#[component]
fn AdminAuctionActionDialog(
    auction_id: String,
    auction_sequence: i64,
    expected_state_version: i64,
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    mut feedback: Signal<Option<bool>>,
    mut notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
    mut cancel_reason: Signal<String>,
    mut command_state: Signal<Option<(String, Uuid)>>,
) -> Element {
    let is_cancel = action == AdminAuctionLifecycleAction::Cancel;
    let is_destructive = matches!(
        action,
        AdminAuctionLifecycleAction::Cancel | AdminAuctionLifecycleAction::FinalPhase
    );
    let can_submit = !busy() && (!is_cancel || !cancel_reason().trim().is_empty());
    rsx! {
        div { class: "admin-auctions__dialog-backdrop", role: "presentation",
            section {
                class: "admin-auctions__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "admin-auction-action-title",
                tabindex: "-1",
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        pending_action.set(None);
                        cancel_reason.set(String::new());
                        feedback.set(None);
                    }
                },
                h2 { id: "admin-auction-action-title",
                    {lifecycle_action_confirm_title(action, auction_sequence)}
                }
                form {
                    class: "admin-auctions__dialog-form",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if !can_submit {
                            return;
                        }
                        let reason = is_cancel.then(|| cancel_reason().trim().to_owned());
                        let auction_id = auction_id.clone();
                        let signature = format!(
                            "{action:?}:{expected_state_version}:{}",
                            reason.as_deref().unwrap_or_default()
                        );
                        let command_id = match command_state() {
                            Some((current, command_id)) if current == signature => command_id,
                            _ => {
                                let command_id = Uuid::new_v4();
                                command_state.set(Some((signature, command_id)));
                                command_id
                            }
                        };
                        let mut busy = busy;
                        let mut feedback = feedback;
                        let mut notice = notice;
                        let mut pending_action = pending_action;
                        let mut cancel_reason = cancel_reason;
                        let mut reload_generation = reload_generation;
                        busy.set(true);
                        spawn(async move {
                            match execute_admin_auction_lifecycle(
                                command_id,
                                auction_id,
                                action,
                                expected_state_version,
                                reason,
                            )
                            .await
                            {
                                Ok(_) => {
                                    feedback.set(None);
                                    notice.set(Some(action));
                                    pending_action.set(None);
                                    cancel_reason.set(String::new());
                                    command_state.set(None);
                                    reload_generation.set(reload_generation().saturating_add(1));
                                }
                                Err(_) => feedback.set(Some(false)),
                            }
                            busy.set(false);
                        });
                    },
                    p { class: "admin-panel__note", {lifecycle_action_confirm_note(action)} }
                    if is_cancel {
                        label {
                            {t!("admin-auctions-cancel-reason")}
                            textarea {
                                maxlength: "1024",
                                required: true,
                                autofocus: true,
                                disabled: busy(),
                                value: cancel_reason(),
                                oninput: move |event| {
                                    cancel_reason.set(event.value());
                                    feedback.set(None);
                                },
                            }
                        }
                        p { class: "admin-panel__note", {t!("admin-auctions-cancel-reason-hint")} }
                    }
                    if is_cancel && cancel_reason().trim().is_empty() {
                        p { class: "admin-panel__note action-feedback action-feedback--error", role: "alert",
                            {t!("admin-auctions-cancel-reason-required")}
                        }
                    }
                    div { class: "admin-auctions__dialog-actions",
                        button {
                            r#type: "button",
                            disabled: busy(),
                            autofocus: !is_cancel,
                            onclick: move |_| {
                                pending_action.set(None);
                                cancel_reason.set(String::new());
                                feedback.set(None);
                            },
                            {t!("admin-auctions-action-close")}
                        }
                        button {
                            class: if is_destructive { "admin-btn-danger" } else { "admin-btn-primary" },
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy() { {t!("admin-action-working")} } else { {lifecycle_action_submit_label(action)} }
                        }
                    }
                }
            }
        }
    }
}
