//! Admin 正式开拍前计划结束时间修改对话框。

use auction_web_shared::legacy_t as t;
use dioxus::{html::Key, prelude::*};
use uuid::Uuid;

use crate::api::update_admin_auction_schedule;
use crate::app::surface::admin_api_error_message;

use super::super::helpers::{datetime_local_value, valid_schedule_end};
use super::model::AdminAuctionNotice;

#[component]
pub(super) fn AdminAuctionScheduleAction(
    auction_id: String,
    auction_sequence: i64,
    expected_state_version: i64,
    scheduled_open_at: String,
    scheduled_end_at: Option<String>,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
) -> Element {
    let mut pending = use_signal(|| false);
    let mut end_value = use_signal(String::new);
    let mut feedback = use_signal(|| None::<String>);
    let mut command_state = use_signal(|| None::<(String, Uuid)>);

    rsx! {
        button {
            class: "admin-btn-secondary",
            r#type: "button",
            disabled: busy(),
            onclick: {
                let scheduled_end_at = scheduled_end_at.clone();
                move |_| {
                    if busy() {
                        return;
                    }
                    notice.set(None);
                    end_value.set(
                        scheduled_end_at
                            .as_deref()
                            .and_then(datetime_local_value)
                            .unwrap_or_default(),
                    );
                    feedback.set(None);
                    command_state.set(None);
                    pending.set(true);
                }
            },
            {t!("admin-auctions-action-schedule")}
        }
        if pending() {
            AdminAuctionScheduleDialog {
                auction_id,
                auction_sequence,
                expected_state_version,
                scheduled_open_at,
                busy,
                notice,
                reload_generation,
                pending,
                end_value,
                feedback,
                command_state,
            }
        }
    }
}

#[component]
fn AdminAuctionScheduleDialog(
    auction_id: String,
    auction_sequence: i64,
    expected_state_version: i64,
    scheduled_open_at: String,
    busy: Signal<bool>,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
    pending: Signal<bool>,
    end_value: Signal<String>,
    feedback: Signal<Option<String>>,
    command_state: Signal<Option<(String, Uuid)>>,
) -> Element {
    let can_submit = !busy() && valid_schedule_end(&scheduled_open_at, &end_value());
    let title_id = format!("schedule-title-{auction_id}");
    let input_id = format!("schedule-end-{auction_id}");
    let signals = ScheduleSignals {
        busy,
        notice,
        reload_generation,
        pending,
        end_value,
        feedback,
        command_state,
    };
    rsx! {
        div { class: "admin-shell__dialog-backdrop", role: "presentation",
            section {
                class: "admin-shell__dialog",
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: title_id.clone(),
                onkeydown: move |event| {
                    if event.key() == Key::Escape && !busy() {
                        close_schedule_dialog(signals);
                    }
                },
                h2 { id: title_id,
                    {t!("admin-auctions-schedule-title", number: auction_sequence)}
                }
                form {
                    class: "admin-auction-open",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if can_submit {
                            run_schedule_action(
                                ScheduleRequest {
                                    auction_id: auction_id.clone(),
                                    expected_state_version,
                                    scheduled_end_at: end_value(),
                                },
                                signals,
                            );
                        }
                    },
                    p { class: "admin-panel__note", {t!("admin-auctions-schedule-note")} }
                    label { r#for: input_id.clone(), {t!("admin-auctions-schedule-end")} }
                    input {
                        id: input_id,
                        r#type: "datetime-local",
                        required: true,
                        autofocus: true,
                        disabled: busy(),
                        value: end_value(),
                        oninput: move |event| {
                            end_value.set(event.value());
                            feedback.set(None);
                            command_state.set(None);
                        },
                    }
                    if !end_value().is_empty() && !valid_schedule_end(&scheduled_open_at, &end_value()) {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            {t!("admin-auctions-schedule-invalid")}
                        }
                    } else if let Some(code) = feedback() {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            {admin_api_error_message(&code)}
                        }
                    }
                    div { class: "admin-shell__dialog-actions",
                        button {
                            r#type: "button",
                            disabled: busy(),
                            onclick: move |_| close_schedule_dialog(signals),
                            {t!("admin-auctions-action-close")}
                        }
                        button {
                            class: "admin-btn-primary",
                            r#type: "submit",
                            disabled: !can_submit,
                            if busy() { {t!("admin-action-working")} } else { {t!("admin-auctions-schedule-submit")} }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ScheduleSignals {
    busy: Signal<bool>,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
    pending: Signal<bool>,
    end_value: Signal<String>,
    feedback: Signal<Option<String>>,
    command_state: Signal<Option<(String, Uuid)>>,
}

struct ScheduleRequest {
    auction_id: String,
    expected_state_version: i64,
    scheduled_end_at: String,
}

fn close_schedule_dialog(mut signals: ScheduleSignals) {
    signals.pending.set(false);
    signals.end_value.set(String::new());
    signals.feedback.set(None);
    signals.command_state.set(None);
}

/// 提交计划修改；命令 ID 在相同版本和结束时间的重试期间保持稳定。
fn run_schedule_action(request: ScheduleRequest, mut signals: ScheduleSignals) {
    let ScheduleRequest { auction_id, expected_state_version, scheduled_end_at } = request;
    let signature = format!("{expected_state_version}:{scheduled_end_at}");
    let command_id = match (signals.command_state)() {
        Some((current, command_id)) if current == signature => command_id,
        _ => {
            let command_id = Uuid::new_v4();
            signals.command_state.set(Some((signature, command_id)));
            command_id
        }
    };
    signals.busy.set(true);
    spawn(async move {
        match update_admin_auction_schedule(
            command_id,
            auction_id,
            scheduled_end_at,
            expected_state_version,
        )
        .await
        {
            Ok(_) => {
                signals.notice.set(Some(AdminAuctionNotice::Schedule));
                signals.pending.set(false);
                signals.end_value.set(String::new());
                signals.feedback.set(None);
                signals.command_state.set(None);
                signals.reload_generation.set((signals.reload_generation)().saturating_add(1));
            }
            Err(code) => signals.feedback.set(Some(code)),
        }
        signals.busy.set(false);
    });
}
