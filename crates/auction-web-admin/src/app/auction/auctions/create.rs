//! Admin 拍卖创建表单。

use dioxus::{html::Key, prelude::*};
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::create_admin_auction;

use super::helpers::valid_auction_schedule;

#[component]
pub(super) fn AdminAuctionLoadError(mut reload_generation: Signal<u64>) -> Element {
    rsx! {
        section {
            class: "admin-panel",
            role: "alert",
            aria_labelledby: "admin-auctions-error-title",
            h1 { id: "admin-auctions-error-title", {t!("page-auctions-title")} }
            p { class: "admin-panel__note", {t!("admin-auctions-error")} }
            button {
                r#type: "button",
                onclick: move |_| reload_generation.set(reload_generation().saturating_add(1)),
                {t!("admin-auctions-retry")}
            }
        }
    }
}

#[component]
pub(super) fn AdminAuctionCreateForm(reload_generation: Signal<u64>) -> Element {
    let mut open = use_signal(|| false);

    rsx! {
        if open() {
            div { class: "admin-auctions__dialog-backdrop", role: "presentation",
                section {
                    class: "admin-auctions__dialog",
                    role: "dialog",
                    aria_modal: "true",
                    aria_labelledby: "admin-auctions-create-title",
                    AdminAuctionCreateFields { reload_generation, open }
                }
            }
        } else {
            button {
                class: "admin-auctions__create-opener admin-btn-primary",
                r#type: "button",
                onclick: move |_| open.set(true),
                {t!("admin-auctions-create-submit")}
            }
        }
    }
}

#[component]
fn AdminAuctionCreateFields(reload_generation: Signal<u64>, mut open: Signal<bool>) -> Element {
    let mut world_id = use_signal(|| "main".to_owned());
    let mut sequence = use_signal(String::new);
    let mut preview_scheduled_at = use_signal(String::new);
    let mut scheduled_open_at = use_signal(String::new);
    let mut scheduled_end_at = use_signal(String::new);
    let mut command_id_state = use_signal(|| None::<Uuid>);
    let mut auction_id_state = use_signal(|| None::<Uuid>);
    let busy = use_signal(|| false);
    let mut feedback = use_signal(|| None::<bool>);
    let mut validation_error = use_signal(|| false);

    rsx! {
        section {
            class: "admin-panel",
            aria_labelledby: "admin-auctions-create-title",
            aria_busy: busy().to_string(),
            tabindex: "-1",
            onkeydown: move |event| {
                if event.key() == Key::Escape && !busy() {
                    open.set(false);
                }
            },
            h2 { id: "admin-auctions-create-title", {t!("admin-auctions-create-title")} }
            p { class: "admin-panel__note", {t!("admin-auctions-create-timezone")} }
            form {
                class: "admin-panel__actions admin-auctions__create-form",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy() {
                        return;
                    }
                    let Ok(sequence_value) = sequence().trim().parse::<i64>() else {
                        validation_error.set(true);
                        feedback.set(Some(false));
                        return;
                    };
                    let world_id = world_id();
                    let preview_scheduled_at = preview_scheduled_at();
                    let scheduled_open_at = scheduled_open_at();
                    let scheduled_end_at = scheduled_end_at();
                    if sequence_value <= 0
                        || !valid_auction_schedule(
                            &preview_scheduled_at,
                            &scheduled_open_at,
                            &scheduled_end_at,
                        )
                    {
                        validation_error.set(true);
                        feedback.set(Some(false));
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
                    let mut busy = busy;
                    let mut feedback = feedback;
                    let mut validation_error = validation_error;
                    let mut reload_generation = reload_generation;
                    let mut open = open;
                    busy.set(true);
                    spawn(async move {
                        let result = create_admin_auction(
                            command_id,
                            auction_id,
                            world_id,
                            sequence_value,
                            preview_scheduled_at,
                            scheduled_open_at,
                            scheduled_end_at,
                        )
                        .await;
                        match result {
                            Ok(_) => {
                                feedback.set(Some(true));
                                validation_error.set(false);
                                reload_generation.set(reload_generation().saturating_add(1));
                                open.set(false);
                            }
                            Err(_) => feedback.set(Some(false)),
                        }
                        busy.set(false);
                    });
                },
                label {
                    r#for: "admin-auctions-create-world",
                    {t!("admin-auctions-create-world")}
                    select {
                        id: "admin-auctions-create-world",
                        disabled: busy(),
                        autofocus: true,
                        value: world_id(),
                        onchange: move |event| {
                            world_id.set(event.value());
                            command_id_state.set(None);
                            auction_id_state.set(None);
                            validation_error.set(false);
                            feedback.set(None);
                        },
                        option { value: "main", {t!("admin-auctions-world-main")} }
                        option { value: "isekai", {t!("admin-auctions-world-isekai")} }
                    }
                }
                label {
                    r#for: "admin-auctions-create-sequence",
                    {t!("admin-auctions-create-sequence")}
                    input {
                        id: "admin-auctions-create-sequence",
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
                }
                label {
                    r#for: "admin-auctions-create-preview",
                    {t!("admin-auctions-create-preview")}
                    input {
                        id: "admin-auctions-create-preview",
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
                }
                label {
                    r#for: "admin-auctions-create-open",
                    {t!("admin-auctions-create-open")}
                    input {
                        id: "admin-auctions-create-open",
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
                }
                label {
                    r#for: "admin-auctions-create-end",
                    {t!("admin-auctions-create-end")}
                    input {
                        id: "admin-auctions-create-end",
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
                }
                button {
                    r#type: "submit",
                    disabled: busy(),
                    if busy() { {t!("admin-action-working")} } else { {t!("admin-auctions-create-submit")} }
                }
                button {
                    r#type: "button",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    {t!("admin-auctions-create-close")}
                }
            }
            if let Some(success) = feedback() {
                p {
                    class: if success {
                        "admin-panel__note action-feedback action-feedback--success"
                    } else {
                        "admin-panel__note action-feedback action-feedback--error"
                    },
                    role: if success { "status" } else { "alert" },
                    {if success {
                        t!("admin-auctions-create-success")
                    } else if validation_error() {
                        t!("admin-auctions-create-invalid")
                    } else {
                        t!("admin-auctions-create-error")
                    }}
                }
            }
        }
    }
}
