//! Admin 正式开拍前预览、开拍与计划结束时间修改对话框。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{normalize_datetime, update_admin_auction_schedule};
use crate::app::surface::admin_api_error_message;

use super::super::helpers::{datetime_local_value, valid_schedule_update};
use super::model::AdminAuctionNotice;

#[component]
pub(super) fn AdminAuctionScheduleAction(
    auction_id: String,
    auction_sequence: i64,
    status: String,
    expected_state_version: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: Option<String>,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
) -> Element {
    let mut pending = use_signal(|| false);
    let mut preview_value = use_signal(String::new);
    let mut open_value = use_signal(String::new);
    let mut end_value = use_signal(String::new);
    let mut feedback = use_signal(|| None::<String>);
    let mut command_state = use_signal(|| None::<(ScheduleRequest, Uuid)>);
    let original_preview_scheduled_at = preview_scheduled_at.clone();
    let original_scheduled_open_at = scheduled_open_at.clone();
    let original_scheduled_end_at = scheduled_end_at.clone().unwrap_or_default();

    rsx! {
        button {
            class: "admin-btn-secondary",
            r#type: "button",
            disabled: busy(),
            onclick: {
                let preview_scheduled_at = preview_scheduled_at.clone();
                let scheduled_open_at = scheduled_open_at.clone();
                let scheduled_end_at = scheduled_end_at.clone();
                move |_| {
                    if busy() {
                        return;
                    }
                    notice.set(None);
                    preview_value.set(
                        datetime_local_value(&preview_scheduled_at).unwrap_or_default(),
                    );
                    open_value.set(
                        datetime_local_value(&scheduled_open_at).unwrap_or_default(),
                    );
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
                status,
                original_preview_scheduled_at,
                original_scheduled_open_at,
                original_scheduled_end_at,
                expected_state_version,
                busy,
                notice,
                reload_generation,
                pending,
                preview_value,
                open_value,
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
    status: String,
    original_preview_scheduled_at: String,
    original_scheduled_open_at: String,
    original_scheduled_end_at: String,
    expected_state_version: i64,
    busy: Signal<bool>,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
    pending: Signal<bool>,
    preview_value: Signal<String>,
    open_value: Signal<String>,
    end_value: Signal<String>,
    feedback: Signal<Option<String>>,
    command_state: Signal<Option<(ScheduleRequest, Uuid)>>,
) -> Element {
    let preview_locked = status == "preview";
    // 先恢复未编辑字段的精度，再校验最终候选；重试直接使用第一次提交的完整请求。
    let request = command_state().map(|(request, _)| request).unwrap_or_else(|| ScheduleRequest {
        auction_id: auction_id.clone(),
        expected_state_version,
        preview_scheduled_at: normalize_datetime(&if preview_locked {
            original_preview_scheduled_at.clone()
        } else {
            preserve_unchanged_datetime(preview_value(), &original_preview_scheduled_at)
        }),
        scheduled_open_at: normalize_datetime(&preserve_unchanged_datetime(
            open_value(),
            &original_scheduled_open_at,
        )),
        scheduled_end_at: normalize_datetime(&preserve_unchanged_datetime(
            end_value(),
            &original_scheduled_end_at,
        )),
    });
    let can_submit = !busy()
        && (command_state().is_some()
            || valid_schedule_update(
                &status,
                &request.preview_scheduled_at,
                &request.scheduled_open_at,
                &request.scheduled_end_at,
            ));
    let preview_input_id = format!("schedule-preview-{auction_id}");
    let open_input_id = format!("schedule-open-{auction_id}");
    let end_input_id = format!("schedule-end-{auction_id}");
    let signals = ScheduleSignals {
        busy,
        notice,
        reload_generation,
        pending,
        preview_value,
        open_value,
        end_value,
        feedback,
        command_state,
    };
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；关闭统一走 close_schedule_dialog。
    rsx! {
        Dialog {
            title: t!("admin-auctions-schedule-title", number: auction_sequence),
            close_label: t!("admin-auctions-action-close"),
            busy: busy(),
            on_close: move |_| close_schedule_dialog(signals),
            form {
                    class: "admin-auction-open",
                    onsubmit: move |event| {
                        event.prevent_default();
                        if can_submit {
                            run_schedule_action(request.clone(), signals);
                        }
                    },
                    p { class: "admin-panel__note", {t!("admin-auctions-schedule-note")} }
                    label { r#for: preview_input_id.clone(), {t!("admin-auctions-schedule-preview")} }
                    input {
                        id: preview_input_id,
                        r#type: "datetime-local",
                        required: true,
                        autofocus: !preview_locked,
                        disabled: busy() || preview_locked,
                        value: preview_value(),
                        oninput: move |event| {
                            preview_value.set(event.value());
                            feedback.set(None);
                            command_state.set(None);
                        },
                    }
                    label { r#for: open_input_id.clone(), {t!("admin-auctions-schedule-open")} }
                    input {
                        id: open_input_id,
                        r#type: "datetime-local",
                        required: true,
                        autofocus: preview_locked,
                        disabled: busy(),
                        value: open_value(),
                        oninput: move |event| {
                            open_value.set(event.value());
                            feedback.set(None);
                            command_state.set(None);
                        },
                    }
                    label { r#for: end_input_id.clone(), {t!("admin-auctions-schedule-end")} }
                    input {
                        id: end_input_id,
                        r#type: "datetime-local",
                        required: true,
                        disabled: busy(),
                        value: end_value(),
                        oninput: move |event| {
                            end_value.set(event.value());
                            feedback.set(None);
                            command_state.set(None);
                        },
                    }
                    if !can_submit && !busy() {
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

#[derive(Clone, Copy)]
struct ScheduleSignals {
    busy: Signal<bool>,
    notice: Signal<Option<AdminAuctionNotice>>,
    reload_generation: Signal<u64>,
    pending: Signal<bool>,
    preview_value: Signal<String>,
    open_value: Signal<String>,
    end_value: Signal<String>,
    feedback: Signal<Option<String>>,
    command_state: Signal<Option<(ScheduleRequest, Uuid)>>,
}

#[derive(Clone, PartialEq)]
struct ScheduleRequest {
    auction_id: String,
    expected_state_version: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
}

fn close_schedule_dialog(mut signals: ScheduleSignals) {
    signals.pending.set(false);
    signals.preview_value.set(String::new());
    signals.open_value.set(String::new());
    signals.end_value.set(String::new());
    signals.feedback.set(None);
    signals.command_state.set(None);
}

/// `datetime-local` 仅显示到分钟；值未被编辑时保留服务端的秒与小数秒精度。
fn preserve_unchanged_datetime(display_value: String, original_rfc3339: &str) -> String {
    if datetime_local_value(original_rfc3339).as_deref() == Some(display_value.as_str()) {
        original_rfc3339.to_owned()
    } else {
        display_value
    }
}

/// 提交计划修改；命令 ID 在相同版本和三项规范 UTC 时间的重试期间保持稳定。
fn run_schedule_action(request: ScheduleRequest, mut signals: ScheduleSignals) {
    let (request, command_id) = (signals.command_state)().unwrap_or_else(|| {
        let command_id = Uuid::new_v4();
        signals.command_state.set(Some((request.clone(), command_id)));
        (request, command_id)
    });
    let ScheduleRequest {
        auction_id,
        expected_state_version,
        preview_scheduled_at,
        scheduled_open_at,
        scheduled_end_at,
    } = request;
    signals.busy.set(true);
    spawn(async move {
        match update_admin_auction_schedule(
            command_id,
            auction_id,
            preview_scheduled_at,
            scheduled_open_at,
            scheduled_end_at,
            expected_state_version,
        )
        .await
        {
            Ok(_) => {
                signals.notice.set(Some(AdminAuctionNotice::Schedule));
                signals.pending.set(false);
                signals.preview_value.set(String::new());
                signals.open_value.set(String::new());
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
