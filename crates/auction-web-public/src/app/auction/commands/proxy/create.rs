//! 代理授权创建表单及其提交状态。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{ProxyCommandAuthorization, create_proxy_authorization};

use super::super::super::{format_amount, parse_credit_amount};
use super::super::common::{
    PendingProxyCommand, ProxyConfirmCommand, amount_hint, initial_proxy_amount,
    proxy_error_message,
};

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
    /// 由弹窗宿主注入的两阶段确认命令 hook（创建实例；见 ProxyFormInput）。
    command: ProxyConfirmCommand,
    /// 另一代理命令的在飞状态；实时快照切换表单后仍必须保持同一宿主互斥。
    peer_submitting: Signal<bool>,
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
    let mut maximum_amount = use_signal(|| initial_proxy_amount(initial_minimum));
    let mut mode = use_signal(|| "fast".to_owned());
    let mut validation = use_signal(|| None::<String>);
    // 两阶段确认状态（command/confirmation/submitting/failure）由宿主注入的 hook 持有。
    let mut command = command;
    let own_submitting = command.submitting;
    let submitting = move || own_submitting() || peer_submitting();
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = lot_status == "live";
    let limit_reached = initial_minimum > MAX_PUBLIC_BID_AMOUNT;
    let blocked = blocked || !bidding_open || limit_reached;
    let blocked_reason = if !bidding_open {
        t!("bid-unavailable")
    } else if limit_reached {
        t!("bid-limit-note")
    } else {
        blocked_reason
    };
    let confirmation_id = format!("public-proxy-create-confirmation-{lot_id}");
    let confirmation_description = match (command.confirmation)() {
        Some(PendingProxyCommand::Create { amount, mode, .. }) => {
            let mode_label =
                if mode == "slow" { t!("proxy-mode-slow") } else { t!("proxy-mode-fast") };
            Some(t!(
                "proxy-create-confirm-description",
                maximum: format_amount(amount),
                mode: mode_label
            ))
        }
        _ => None,
    };
    let confirming = confirmation_description.is_some();

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
                    // 首次提交只冻结金额、模式与 command_id 并进入确认态；确认态再次提交后
                    // 才调用外部 API。签名取 (amount, mode)：同输入重试复用同一 command_id。
                    let pending = if let Some(pending) = (command.confirmation)() {
                        match &pending {
                            PendingProxyCommand::Create { amount, mode, .. } => {
                                if *amount > MAX_PUBLIC_BID_AMOUNT {
                                    command.cancel_confirmation();
                                    validation.set(Some(t!("proxy-invalid-amount")));
                                    (command.failure).set(None);
                                    return;
                                }
                                if *amount < initial_minimum {
                                    command.cancel_confirmation();
                                    validation.set(Some(t!(
                                        "proxy-below-minimum",
                                        price: format_amount(initial_minimum)
                                    )));
                                    (command.failure).set(None);
                                    return;
                                }
                                if mode == "slow" && !allow_slow {
                                    command.cancel_confirmation();
                                    validation.set(Some(t!("proxy-slow-forbidden")));
                                    (command.failure).set(None);
                                    return;
                                }
                            }
                            // 本表单只冻结 Create；防御性处理串型，退回编辑态。
                            PendingProxyCommand::Lower { .. } => {
                                command.reset();
                                return;
                            }
                        }
                        pending
                    } else {
                        let Some(value) = parse_credit_amount(&maximum_amount()) else {
                            validation.set(Some(t!("proxy-invalid-amount")));
                            (command.failure).set(None);
                            return;
                        };
                        if value < initial_minimum {
                            validation.set(Some(t!(
                                "proxy-below-minimum",
                                price: format_amount(initial_minimum)
                            )));
                            (command.failure).set(None);
                            return;
                        }
                        let selected_mode = mode();
                        if selected_mode == "slow" && !allow_slow {
                            validation.set(Some(t!("proxy-slow-forbidden")));
                            (command.failure).set(None);
                            return;
                        }
                        command.begin(
                            format!("{value}:{selected_mode}"),
                            |command_id| PendingProxyCommand::Create {
                                amount: value,
                                mode: selected_mode,
                                command_id,
                            },
                        );
                        validation.set(None);
                        return;
                    };
                    validation.set(None);
                    // 上面的确认守卫已保证此处必为 Create。
                    let PendingProxyCommand::Create { amount, mode, command_id } =
                        pending.clone()
                    else {
                        return;
                    };
                    let lot_id = lot_id.clone();
                    command.submit(
                        pending,
                        move |_| {
                            let lot_id = lot_id.clone();
                            async move {
                                create_proxy_authorization(lot_id, amount, mode, command_id).await
                            }
                        },
                        crate::api::CommandError::outcome_uncertain,
                        move |authorization| {
                            saved_authorization.set(Some(Box::new(authorization)));
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        },
                        move || {
                            // 失败后 hook 保留输入、command_id 和错误，这里重新读取权威价格与代理状态。
                            refresh_generation.set(refresh_generation().saturating_add(1));
                        },
                    );
                },
                if let Some(description) = confirmation_description {
                    p {
                        id: confirmation_id.clone(),
                        class: "public-auction-bid-form__confirmation",
                        role: "status",
                        aria_live: "polite",
                        aria_atomic: "true",
                        "{description}"
                    }
                } else {
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
                }
                if blocked {
                    p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
                }
                if let Some(message) = validation() {
                    p { class: "action-feedback action-feedback--error", role: "alert", "{message}" }
                }
                if let Some(error) = (command.failure)() {
                    p { class: "action-feedback action-feedback--error", role: "alert",
                        "{proxy_error_message(&error, initial_minimum)}"
                    }
                }
                div { class: "public-auction-bid-form__actions",
                    button {
                        r#type: "button",
                        disabled: submitting(),
                        onclick: move |_| {
                            if (command.confirmation)().is_some() {
                                command.cancel_confirmation();
                            } else {
                                on_close.call(());
                            }
                        },
                        if confirming { {t!("proxy-confirm-back")} } else { {t!("dialog-cancel")} }
                    }
                    button {
                        r#type: "submit",
                        disabled: submitting() || blocked,
                        aria_describedby: if confirming { confirmation_id.clone() } else { String::new() },
                        if submitting() {
                            {t!("proxy-working")}
                        } else if confirming {
                            {t!("proxy-create-confirm-submit")}
                        } else {
                            {t!("proxy-create-submit")}
                        }
                    }
                }
            }
        }
    }
}
