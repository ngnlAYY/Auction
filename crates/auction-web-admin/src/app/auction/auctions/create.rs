//! Admin 拍卖创建表单。

use auction_web_shared::{Dialog, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::create_admin_auction;
use crate::app::surface::{AdminActionFeedback, run_admin_action};

use super::actions::AdminAuctionNotice;
use super::helpers::valid_auction_schedule;

#[component]
pub(super) fn AdminAuctionCreateForm(
    reload_generation: Signal<u64>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let mut open = use_signal(|| false);
    let busy = use_signal(|| false);

    rsx! {
        button {
            class: "admin-btn-primary admin-auction-create-opener",
            r#type: "button",
            onclick: move |_| {
                if !open() {
                    // 打开新流程时清掉上一次的页面级成功提示。
                    notice.set(None);
                    open.set(true);
                }
            },
            {t!("admin-auctions-create-opener")}
        }
        if open() {
            // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；aria_busy 改挂到表单上。
            Dialog {
                title: t!("admin-auctions-create-title"),
                close_label: t!("admin-auctions-create-close"),
                busy: busy(),
                on_close: move |_| open.set(false),
                AdminAuctionCreateFields { reload_generation, open, busy, notice }
            }
        }
    }
}

#[component]
fn AdminAuctionCreateFields(
    reload_generation: Signal<u64>,
    mut open: Signal<bool>,
    busy: Signal<bool>,
    mut notice: Signal<Option<AdminAuctionNotice>>,
) -> Element {
    let mut world_id = use_signal(String::new);
    let mut sequence = use_signal(|| "1".to_owned());
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let mut auction_id_state = use_signal(|| None::<Uuid>);
    let mut feedback = use_signal(|| None::<AdminActionFeedback>);
    let mut validation_error = use_signal(|| false);

    rsx! {
        Fragment {
            form {
            class: "admin-auction-create",
            aria_busy: busy().to_string(),
            onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let Ok(sequence_value) = sequence().trim().parse::<i64>() else {
                        validation_error.set(true);
                        feedback.set(Some(AdminActionFeedback::Error("invalid_request".to_owned())));
                        return;
                    };
                    let world_id = world_id();
                    let preview_scheduled_at = preview_scheduled_at();
                    let scheduled_open_at = scheduled_open_at();
                    let scheduled_end_at = scheduled_end_at();
                    if world_id.trim().is_empty()
                        || sequence_value <= 0
                        || !valid_auction_schedule(
                            &preview_scheduled_at,
                            &scheduled_open_at,
                            &scheduled_end_at,
                        )
                    {
                        validation_error.set(true);
                        feedback.set(Some(AdminActionFeedback::Error("invalid_request".to_owned())));
                        return;
                    }
                    let command_id = command_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        command_id_state.set(Some(value));
                        value
                    });
                    let auction_id = auction_id_state().unwrap_or_else(|| {
                        let value = Uuid::new_v4();
                        auction_id_state.set(Some(value));
                        value
                    });
                    let mut feedback = feedback;
                    let mut validation_error = validation_error;
                    let mut reload_generation = reload_generation;
                    let mut notice = notice;
                    let mut open = open;
                    run_admin_action(
                        busy,
                        async move {
                            create_admin_auction(
                                command_id,
                                auction_id,
                                world_id,
                                sequence_value,
                                preview_scheduled_at,
                                scheduled_open_at,
                                scheduled_end_at,
                            )
                            .await
                        },
                        move |_| {
                            // 对话框内反馈随关闭一并销毁；成功提示移交页面级 notice。
                            feedback.set(None);
                            notice.set(Some(AdminAuctionNotice::Created));
                            validation_error.set(false);
                            reload_generation.set(reload_generation().saturating_add(1));
                            open.set(false);
                        },
                        move |code| feedback.set(Some(AdminActionFeedback::Error(code))),
                    );
                },
                p { class: "admin-panel__note", {t!("admin-auctions-create-timezone")} }
                label {
                    r#for: "create-world",
                    {t!("admin-auctions-create-world")}
                }
                select {
                    id: "create-world",
                    disabled: busy(),
                    autofocus: true,
                    required: true,
                    value: world_id(),
                    onchange: move |event| {
                        world_id.set(event.value());
                        command_id_state.set(None);
                        auction_id_state.set(None);
                        validation_error.set(false);
                        feedback.set(None);
                    },
                    option { value: "", disabled: true, {t!("value-empty")} }
                    option { value: "main", {t!("admin-auctions-world-main")} }
                    option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                }
                label {
                    r#for: "create-sequence",
                    {t!("admin-auctions-create-sequence")}
                }
                input {
                    id: "create-sequence",
                    r#type: "number",
                    min: "1",
                    required: true,
                    disabled: busy(),
                    value: sequence(),
                    oninput: move |event| {
                        sequence.set(event.value());
                        command_id_state.set(None);
                        auction_id_state.set(None);
                        validation_error.set(false);
                        feedback.set(None);
                    },
                }
                label {
                    r#for: "create-preview",
                    {t!("admin-auctions-create-preview")}
                }
                input {
                    id: "create-preview",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    value: preview_scheduled_at(),
                    oninput: move |event| {
                        preview_scheduled_at.set(event.value());
                        command_id_state.set(None);
                        auction_id_state.set(None);
                        validation_error.set(false);
                        feedback.set(None);
                    },
                }
                label {
                    r#for: "create-open",
                    {t!("admin-auctions-create-open")}
                }
                input {
                    id: "create-open",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    value: scheduled_open_at(),
                    oninput: move |event| {
                        scheduled_open_at.set(event.value());
                        command_id_state.set(None);
                        auction_id_state.set(None);
                        validation_error.set(false);
                        feedback.set(None);
                    },
                }
                label {
                    r#for: "create-end",
                    {t!("admin-auctions-create-end")}
                }
                input {
                    id: "create-end",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy(),
                    value: scheduled_end_at(),
                    oninput: move |event| {
                        scheduled_end_at.set(event.value());
                        command_id_state.set(None);
                        auction_id_state.set(None);
                        validation_error.set(false);
                        feedback.set(None);
                    },
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-auctions-create-submit")} }
                }
                if let Some(feedback) = feedback() {
                    p {
                        class: if feedback.is_success() {
                            "admin-panel__note action-feedback action-feedback--success"
                        } else {
                            "admin-panel__note action-feedback action-feedback--error"
                        },
                        role: if feedback.is_success() { "status" } else { "alert" },
                        {if feedback.is_success() {
                            t!("admin-auctions-create-success")
                        } else if validation_error() {
                            t!("admin-auctions-create-invalid")
                        } else {
                            feedback.message(t!("admin-auctions-create-success"))
                        }}
                    }
                }
            }
            div { class: "admin-shell__dialog-actions",
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    {t!("admin-auctions-create-close")}
                }
            }
        }
    }
}
