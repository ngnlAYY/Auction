//! 代理授权创建表单及其提交状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{CommandError, ProxyCommandAuthorization, create_proxy_authorization};

use super::super::super::{format_amount, parse_credit_amount};
use super::super::common::{PendingProxyCommand, amount_hint, proxy_error_message};

#[component]
pub(super) fn ProxyCreateForm(
    lot_id: String,
    current_price: i64,
    next_bid: i64,
    initial_minimum: i64,
    lot_status: String,
    allow_slow: bool,
    blocked: bool,
    blocked_reason: String,
    mut saved_authorization: Signal<Option<Box<ProxyCommandAuthorization>>>,
    on_close: EventHandler<()>,
) -> Element {
    let maximum_id = format!("public-proxy-maximum-{lot_id}");
    let mode_name = format!("public-proxy-mode-{lot_id}");
    let fast_mode_id = format!("{mode_name}-fast");
    let slow_mode_id = format!("{mode_name}-slow");
    let fast_hint_id = format!("{mode_name}-fast-hint");
    let slow_hint_id = format!("{mode_name}-slow-hint");
    let slow_reason_id = format!("{mode_name}-slow-disabled");
    let slow_described_by =
        if allow_slow { slow_hint_id.clone() } else { format!("{slow_hint_id} {slow_reason_id}") };
    let mut maximum_amount = use_signal(|| format_amount(initial_minimum));
    let mut mode = use_signal(|| "fast".to_owned());
    let mut failure = use_signal(|| None::<CommandError>);
    let mut validation = use_signal(|| None::<String>);
    let mut submitting = use_signal(|| false);
    let mut command = use_signal(|| None::<PendingProxyCommand>);
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = lot_status == "live";
    let blocked = blocked || !bidding_open;
    let blocked_reason = if bidding_open { blocked_reason } else { t!("bid-unavailable") };

    rsx! {
        section {
            class: "public-auction-bid-form",
            aria_busy: submitting().to_string(),
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    if submitting() || blocked {
                        return;
                    }
                    let Some(value) = parse_credit_amount(&maximum_amount()) else {
                        validation.set(Some(t!("proxy-invalid-amount")));
                        failure.set(None);
                        return;
                    };
                    if value < initial_minimum {
                        validation.set(Some(t!("proxy-below-minimum", price: format_amount(initial_minimum))));
                        failure.set(None);
                        return;
                    }
                    let selected_mode = mode();
                    if selected_mode == "slow" && !allow_slow {
                        validation.set(Some(t!("proxy-slow-forbidden")));
                        failure.set(None);
                        return;
                    }
                    let pending = match command() {
                        Some(PendingProxyCommand::Create { amount, mode, command_id })
                            if amount == value && mode == selected_mode => {
                                PendingProxyCommand::Create { amount, mode, command_id }
                            }
                        _ => PendingProxyCommand::Create {
                            amount: value,
                            mode: selected_mode,
                            command_id: Uuid::new_v4(),
                        },
                    };
                    command.set(Some(pending.clone()));
                    validation.set(None);
                    failure.set(None);
                    submitting.set(true);
                    let lot_id = lot_id.clone();
                    let (amount, mode, command_id) = match pending {
                        PendingProxyCommand::Create { amount, mode, command_id } => {
                            (amount, mode, command_id)
                        }
                        PendingProxyCommand::Lower { .. } => return,
                    };
                    spawn(async move {
                        let command_result =
                            create_proxy_authorization(lot_id, amount, mode, command_id).await;
                        submitting.set(false);
                        match command_result {
                            Ok(authorization) => {
                                command.set(None);
                                saved_authorization.set(Some(Box::new(authorization)));
                                refresh_generation.set(refresh_generation().saturating_add(1));
                            }
                            Err(error) => {
                                failure.set(Some(error));
                                // 失败后保留输入、command_id 和错误，同时重新读取权威价格与代理状态。
                                refresh_generation.set(refresh_generation().saturating_add(1));
                            }
                        }
                    });
                },
                p { class: "public-auction-bid-form__meta",
                    {t!("bid-current-price", price: format_amount(current_price))}
                    " · "
                    {t!("bid-minimum", price: format_amount(initial_minimum))}
                }
                label { for: maximum_id.clone(), {t!("proxy-create-maximum-label")} }
                input {
                    id: maximum_id,
                    r#type: "text",
                    autofocus: true,
                    inputmode: "decimal",
                    aria_describedby: "public-proxy-amount-hint",
                    disabled: submitting() || blocked,
                    value: "{maximum_amount}",
                    oninput: move |event| maximum_amount.set(event.value()),
                }
                {amount_hint(
                    &maximum_amount(),
                    "public-proxy-amount-hint",
                    "proxy-invalid-amount",
                )}
                p { class: "public-auction-bid-form__meta", {t!("proxy-maximum-hint")} }
                p { class: "public-auction-bid-form__hint", {t!("proxy-shorthand-hint")} }
                fieldset {
                    disabled: submitting() || blocked,
                    legend { {t!("proxy-create-mode-label")} }
                    div { class: "public-auction-bid-form__mode",
                        input {
                            id: fast_mode_id.clone(),
                            r#type: "radio",
                            name: mode_name.clone(),
                            value: "fast",
                            checked: mode() == "fast",
                            aria_describedby: fast_hint_id.clone(),
                            onchange: move |_| mode.set("fast".to_owned()),
                        }
                        label { for: fast_mode_id, {t!("proxy-mode-fast")} }
                        p { id: fast_hint_id, class: "public-auction-bid-form__mode-hint",
                            {t!("proxy-mode-fast-hint")}
                        }
                    }
                    div { class: "public-auction-bid-form__mode",
                        input {
                            id: slow_mode_id.clone(),
                            r#type: "radio",
                            name: mode_name,
                            value: "slow",
                            checked: mode() == "slow",
                            disabled: !allow_slow,
                            aria_describedby: slow_described_by,
                            onchange: move |_| mode.set("slow".to_owned()),
                        }
                        label { for: slow_mode_id, {t!("proxy-mode-slow")} }
                        p { id: slow_hint_id, class: "public-auction-bid-form__mode-hint",
                            {t!("proxy-mode-slow-hint")}
                        }
                    }
                }
                if !allow_slow {
                    p { id: slow_reason_id, class: "public-auction-bid-form__hint", role: "note",
                        {t!("proxy-slow-forbidden")}
                    }
                }
                if blocked {
                    p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
                }
                if let Some(message) = validation() {
                    p { class: "action-feedback action-feedback--error", role: "alert", "{message}" }
                }
                if let Some(error) = failure() {
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        "{proxy_error_message(&error, initial_minimum)}"
                    }
                }
                div { class: "public-auction-bid-form__actions",
                    button {
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("dialog-cancel")}
                    }
                    button { r#type: "submit", disabled: submitting() || blocked,
                        if submitting() { {t!("proxy-working")} } else { {t!("proxy-create-submit")} }
                    }
                }
            }
        }
    }
}
