//! Admin 拍卖生命周期操作与确认对话框。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionLifecycleAction,
    execute_admin_auction_lifecycle,
};

use super::super::{Route, format_admin_time};
use super::helpers::{
    admin_auction_status_label, admin_auction_world_label, scheduled_time_millis,
};

#[component]
pub(super) fn AdminAuctionRow(
    auction: AdminAuction,
    capabilities: AdminAuctionCapabilities,
    clock_millis: Signal<i64>,
    notice: Signal<Option<AdminAuctionLifecycleAction>>,
    reload_generation: Signal<u64>,
) -> Element {
    let auction_id = auction.id.clone();
    let lifecycle_status = auction.status.clone();
    let current_time_millis = clock_millis();
    let open_ready = auction.status == "preview"
        && (capabilities.manual_early_open
            || scheduled_time_millis(&auction.scheduled_open_at)
                .is_some_and(|scheduled| scheduled <= current_time_millis));
    rsx! {
        tr { class: "admin-auctions__row",
            td { class: "public-auction-catalog__number", "data-label": t!("admin-auctions-sequence"),
                h3 { class: "admin-visually-hidden",
                    {t!("admin-auctions-row-heading", number: auction.sequence, world: auction.world_id.clone())}
                }
                "#{auction.sequence}"
            }
            td { "data-label": t!("admin-auctions-world"), {admin_auction_world_label(&auction.world_id)} }
            td { "data-label": t!("admin-auctions-status"),
                span { class: "admin-auctions__status admin-auctions__status--{auction.status}",
                    {admin_auction_status_label(&auction.status)}
                }
            }
            td { "data-label": t!("admin-auctions-open-at"), time { datetime: auction.scheduled_open_at.clone(), "{format_admin_time(&auction.scheduled_open_at)}" } }
            td {
                "data-label": t!("admin-auctions-end-at"),
                if let Some(ends_at) = auction.scheduled_ends_at.clone().or(auction.scheduled_end_at.clone()) {
                    time { datetime: ends_at.clone(), "{format_admin_time(&ends_at)}" }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-auctions-readiness"),
                if open_ready {
                    span { class: "admin-auctions__ready", {t!("admin-auctions-ready")} }
                } else {
                    {t!("value-empty")}
                }
            }
            td { "data-label": t!("admin-auctions-actions"),
                AdminAuctionActions {
                    auction_id,
                    auction_sequence: auction.sequence,
                    status: lifecycle_status,
                    expected_state_version: auction.state_version,
                    capabilities,
                    scheduled_open_at: auction.scheduled_open_at.clone(),
                    current_time_millis,
                    notice,
                    reload_generation,
                }
            }
        }
    }
}

#[component]
fn AdminAuctionActions(
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
                to: Route::AuctionLots { auction_id: auction_id.clone() },
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
                                        reload_generation
                                            .set(reload_generation().saturating_add(1));
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

fn lifecycle_actions(
    status: &str,
    capabilities: &AdminAuctionCapabilities,
    scheduled_open_at: &str,
    current_time_millis: i64,
) -> Vec<AdminAuctionLifecycleAction> {
    let mut actions = Vec::new();
    match status {
        "draft" => {
            if capabilities.manual_preview_open {
                actions.push(AdminAuctionLifecycleAction::Preview);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "preview" => {
            if capabilities.manual_early_open
                || scheduled_time_millis(scheduled_open_at)
                    .is_some_and(|scheduled| scheduled <= current_time_millis)
            {
                actions.push(AdminAuctionLifecycleAction::Open);
            }
            if capabilities.cancel_auction {
                actions.push(AdminAuctionLifecycleAction::Cancel);
            }
        }
        "live" => {
            actions.push(AdminAuctionLifecycleAction::FinalPhase);
        }
        _ => {}
    }
    actions
}

fn lifecycle_action_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final"),
    }
}

fn lifecycle_action_button_class(action: AdminAuctionLifecycleAction) -> &'static str {
    match action {
        AdminAuctionLifecycleAction::Preview | AdminAuctionLifecycleAction::Open => {
            "admin-btn-primary"
        }
        AdminAuctionLifecycleAction::Cancel | AdminAuctionLifecycleAction::FinalPhase => {
            "admin-btn-danger"
        }
    }
}

fn lifecycle_action_submit_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-confirm"),
        _ => lifecycle_action_label(action),
    }
}

fn lifecycle_action_confirm_title(
    action: AdminAuctionLifecycleAction,
    auction_sequence: i64,
) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => {
            t!("admin-auctions-confirm-preview", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Open => {
            t!("admin-auctions-confirm-open", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::Cancel => {
            t!("admin-auctions-confirm-cancel", number: auction_sequence)
        }
        AdminAuctionLifecycleAction::FinalPhase => {
            t!("admin-auctions-confirm-final", number: auction_sequence)
        }
    }
}

fn lifecycle_action_confirm_note(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-confirm-preview-note"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-confirm-open-note"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-confirm-cancel-note"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-confirm-final-note"),
    }
}

pub(super) fn lifecycle_action_success_label(action: AdminAuctionLifecycleAction) -> String {
    match action {
        AdminAuctionLifecycleAction::Preview => t!("admin-auctions-action-preview-success"),
        AdminAuctionLifecycleAction::Open => t!("admin-auctions-action-open-success"),
        AdminAuctionLifecycleAction::Cancel => t!("admin-auctions-action-cancel-success"),
        AdminAuctionLifecycleAction::FinalPhase => t!("admin-auctions-action-final-success"),
    }
}
