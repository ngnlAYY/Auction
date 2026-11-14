//! Admin 拍卖生命周期动作按钮与确认对话框。

use auction_web_shared::{Dialog, t, use_second_tick};
use dioxus::prelude::*;

use super::super::presentation::current_time_millis;
use super::model::{
    AdminAuctionNotice, lifecycle_action_button_class, lifecycle_action_confirm_note,
    lifecycle_action_confirm_title, lifecycle_action_label, lifecycle_action_submit_label,
    lifecycle_actions,
};
use super::schedule::AdminAuctionScheduleAction;
use crate::api::{
    AdminAuctionCapabilities, AdminAuctionLifecycleAction, execute_admin_auction_lifecycle,
};
use crate::surface::admin_api_error_message;
use crate::surface::pending::{PendingCommands, set_if_alive};

#[component]
pub(super) fn AdminAuctionActions(
    auction_id: String,
    auction_sequence: i64,
    status: String,
    expected_state_version: i64,
    capabilities: AdminAuctionCapabilities,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: Option<String>,
    notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    // 秒级节拍订阅下沉到本动作单元格：到时自动出现的「开拍」入口每秒只在本叶子组件
    // 内重算，整行与整表不随秒针重渲染。
    let second_tick = use_second_tick();
    let _ = second_tick();
    let current_time_millis = current_time_millis();
    let busy = use_signal(|| false);
    let feedback = use_signal(|| None::<String>);
    let pending_action = use_signal(|| None::<AdminAuctionLifecycleAction>);
    let action_reason = use_signal(String::new);
    let owner = use_context::<PendingCommands>();
    let blocked = owner.contains(&format!("auction:{auction_id}"));
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
                    blocked,
                    notice,
                    pending_action,
                }
            }
            if capabilities.modify_schedule && matches!(status.as_str(), "draft" | "preview") {
                AdminAuctionScheduleAction {
                    auction_id: auction_id.clone(),
                    auction_sequence,
                    status: status,
                    expected_state_version,
                    preview_scheduled_at: preview_scheduled_at,
                    scheduled_open_at: scheduled_open_at,
                    scheduled_end_at,
                    busy,
                    notice,
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
                pending_action,
                action_reason,
            }
        }
        if let Some(code) = feedback() {
            span {
                class: "admin-panel__note action-feedback action-feedback--error",
                role: "alert",
                {admin_api_error_message(&code)}
            }
        }
    }
}

#[component]
fn AdminAuctionActionButton(
    action: AdminAuctionLifecycleAction,
    busy: Signal<bool>,
    blocked: bool,
    mut notice: Signal<Option<AdminAuctionNotice>>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
) -> Element {
    rsx! {
        button {
            class: lifecycle_action_button_class(action),
            r#type: "button",
            disabled: busy() || blocked,
            onclick: move |_| {
                if busy() || blocked {
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
    mut feedback: Signal<Option<String>>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
    mut pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
    mut action_reason: Signal<String>,
) -> Element {
    let owner = use_context::<PendingCommands>();
    let is_cancel = action == AdminAuctionLifecycleAction::Cancel;
    let is_final = action == AdminAuctionLifecycleAction::FinalPhase;
    let is_early_settlement = action == AdminAuctionLifecycleAction::EarlySettlement;
    let requires_reason = is_cancel || is_early_settlement;
    let is_destructive = matches!(
        action,
        AdminAuctionLifecycleAction::Cancel
            | AdminAuctionLifecycleAction::FinalPhase
            | AdminAuctionLifecycleAction::EarlySettlement
    );
    let can_submit = !busy()
        && !owner.contains(&format!("auction:{auction_id}"))
        && (!requires_reason || !action_reason().trim().is_empty());
    let action_reason_id = format!("action-reason-{auction_id}");
    let action_signals =
        LifecycleActionSignals { owner, feedback, notice, pending_action, action_reason };
    // 外壳（遮罩/标题栏/三条关闭路径）由共享 Dialog 接管；Escape 与关闭按钮统一走
    // close_lifecycle_dialog，清理语义与旧的 Escape 处理器一致。
    rsx! {
    Dialog {
        title: lifecycle_action_confirm_title(action, auction_sequence),
        close_label: t!("admin-auctions-action-close"),
        busy: busy(),
        on_close: move |_| close_lifecycle_dialog(action_signals),
        if is_final {
                p { {lifecycle_action_confirm_note(action)} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        disabled: busy(),
                        autofocus: true,
                        onclick: move |_| close_lifecycle_dialog(action_signals),
                        {t!("admin-auctions-action-close")}
                    }
                    button {
                        class: "admin-btn-danger",
                        r#type: "button",
                        disabled: !can_submit,
                        onclick: move |_| run_lifecycle_action(
                            LifecycleActionRequest {
                                auction_id: auction_id.clone(),
                                expected_state_version,
                                action,
                                reason: None,
                            },
                            action_signals,
                        ),
                        if busy() { {t!("admin-action-working")} } else { {lifecycle_action_submit_label(action)} }
                    }
                }
            } else {
                form {
                    class: "admin-auction-open",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if !can_submit {
                            return;
                        }
                        run_lifecycle_action(
                            LifecycleActionRequest {
                                auction_id: auction_id.clone(),
                                expected_state_version,
                                action,
                                reason: requires_reason
                                    .then(|| action_reason().trim().to_owned()),
                            },
                            action_signals,
                        );
                    },
                    p { {lifecycle_action_confirm_note(action)} }
                    if requires_reason {
                        label {
                            r#for: action_reason_id.clone(),
                            if is_cancel {
                                {t!("admin-auctions-cancel-reason")}
                            } else {
                                {t!("admin-auctions-early-settlement-reason")}
                            }
                        }
                        textarea {
                            id: action_reason_id,
                            maxlength: "1024",
                            required: true,
                            autofocus: true,
                            disabled: busy(),
                            value: action_reason(),
                            oninput: move |event| {
                                action_reason.set(event.value());
                                feedback.set(None);
                            },
                        }
                        p {
                            if is_cancel {
                                {t!("admin-auctions-cancel-reason-hint")}
                            } else {
                                {t!("admin-auctions-early-settlement-reason-hint")}
                            }
                        }
                    }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            r#type: "button",
                            disabled: busy(),
                            autofocus: !requires_reason,
                            onclick: move |_| close_lifecycle_dialog(action_signals),
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

#[derive(Clone, Copy)]
struct LifecycleActionSignals {
    owner: PendingCommands,
    feedback: Signal<Option<String>>,
    notice: Signal<Option<AdminAuctionNotice>>,
    pending_action: Signal<Option<AdminAuctionLifecycleAction>>,
    action_reason: Signal<String>,
}

#[derive(Clone)]
struct LifecycleActionRequest {
    auction_id: String,
    expected_state_version: i64,
    action: AdminAuctionLifecycleAction,
    reason: Option<String>,
}

/// 关闭生命周期对话框并丢弃只属于当前动作的表单状态。
fn close_lifecycle_dialog(mut signals: LifecycleActionSignals) {
    signals.pending_action.set(None);
    signals.action_reason.set(String::new());
    signals.feedback.set(None);
}

/// 页宿主拥有首次请求与任务；重试只从常驻未决区发起，不重新读取当前行版本。
fn run_lifecycle_action(request: LifecycleActionRequest, signals: LifecycleActionSignals) {
    let action = request.action;
    if signals.owner.submit(
        format!("auction:{}", request.auction_id),
        lifecycle_action_label(action),
        move |command_id| {
            let request = request.clone();
            async move {
                execute_admin_auction_lifecycle(
                    command_id,
                    request.auction_id,
                    request.action,
                    request.expected_state_version,
                    request.reason,
                )
                .await
            }
        },
        move |_| {
            set_if_alive(signals.feedback, None);
            set_if_alive(signals.notice, Some(AdminAuctionNotice::Lifecycle(action)));
        },
        move |code| set_if_alive(signals.feedback, Some(code)),
    ) {
        close_lifecycle_dialog(signals);
    }
}
