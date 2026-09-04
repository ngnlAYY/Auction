//! 已有代理授权的降低上限表单及其提交状态。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::{CommandError, OwnedProxy, ProxyCommandAuthorization, lower_proxy_authorization};

use super::super::super::proxy::proxy_status_label;
use super::super::super::{format_amount, parse_credit_amount};
use super::super::common::{PendingProxyCommand, amount_hint, proxy_error_message};

#[component]
pub(super) fn ProxyAuthorizationForm(
    lot_id: String,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    proxy: OwnedProxy,
    mut saved_authorization: Signal<Option<Box<ProxyCommandAuthorization>>>,
    on_close: EventHandler<()>,
) -> Element {
    let maximum_id = format!("public-proxy-lower-maximum-{lot_id}");
    let lower_bound = if proxy.capabilities.waiting { next_bid } else { current_price };
    let mut maximum_amount = use_signal(|| format_amount(lower_bound));
    let mut failure = use_signal(|| None::<CommandError>);
    let mut validation = use_signal(|| None::<String>);
    let mut submitting = use_signal(|| false);
    let mut command = use_signal(|| None::<PendingProxyCommand>);
    let mut confirmation = use_signal(|| None::<PendingProxyCommand>);
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = lot_status == "live";
    let can_lower = proxy.capabilities.can_lower && bidding_open;
    let blocked_reason =
        if !bidding_open { t!("bid-unavailable") } else { t!("proxy-cannot-lower") };
    let confirmation_id = format!("public-proxy-lower-confirmation-{lot_id}");
    let confirmation_description = match confirmation() {
        Some(PendingProxyCommand::Lower { amount, .. }) => Some(t!(
            "proxy-lower-confirm-description",
            maximum: format_amount(amount)
        )),
        _ => None,
    };
    let confirming = confirmation_description.is_some();

    rsx! {
        section {
            class: "public-auction-bid-form",
            aria_busy: submitting().to_string(),
            h2 { {t!("proxy-current")} }
            p { {t!("proxy-status", status: proxy_status_label(&proxy.status))} }
            p { {t!("proxy-maximum", maximum: format_amount(proxy.maximum_amount))} }
            if can_lower {
                form {
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting() || !can_lower {
                            return;
                        }
                        // 首次提交只冻结新上限与 command_id；确认态再次提交后才调用外部 API。
                        let pending = if let Some(pending) = confirmation() {
                            match &pending {
                                PendingProxyCommand::Lower {
                                    amount,
                                    authorization_id,
                                    ..
                                } => {
                                    if authorization_id != &proxy.authorization_id {
                                        confirmation.set(None);
                                        command.set(None);
                                        validation.set(Some(t!("proxy-authorization-not-active")));
                                        failure.set(None);
                                        return;
                                    }
                                    if *amount >= proxy.maximum_amount {
                                        confirmation.set(None);
                                        validation.set(Some(t!("proxy-not-lower")));
                                        failure.set(None);
                                        return;
                                    }
                                    if *amount < lower_bound {
                                        confirmation.set(None);
                                        validation.set(Some(t!(
                                            "proxy-below-minimum",
                                            price: format_amount(lower_bound)
                                        )));
                                        failure.set(None);
                                        return;
                                    }
                                }
                                PendingProxyCommand::Create { .. } => {
                                    confirmation.set(None);
                                    command.set(None);
                                    return;
                                }
                            }
                            pending
                        } else {
                            let Some(value) = parse_credit_amount(&maximum_amount()) else {
                                validation.set(Some(t!("proxy-invalid-amount")));
                                failure.set(None);
                                return;
                            };
                            if value >= proxy.maximum_amount {
                                validation.set(Some(t!("proxy-not-lower")));
                                failure.set(None);
                                return;
                            }
                            if value < lower_bound {
                                validation.set(Some(t!(
                                    "proxy-below-minimum",
                                    price: format_amount(lower_bound)
                                )));
                                failure.set(None);
                                return;
                            }
                            let authorization_id = proxy.authorization_id.clone();
                            let pending = match command() {
                                Some(PendingProxyCommand::Lower {
                                    amount,
                                    authorization_id: existing_id,
                                    command_id,
                                }) if amount == value && existing_id == authorization_id => {
                                    PendingProxyCommand::Lower {
                                        amount,
                                        authorization_id,
                                        command_id,
                                    }
                                }
                                _ => PendingProxyCommand::Lower {
                                    amount: value,
                                    authorization_id,
                                    command_id: Uuid::new_v4(),
                                },
                            };
                            command.set(Some(pending.clone()));
                            confirmation.set(Some(pending));
                            validation.set(None);
                            failure.set(None);
                            return;
                        };
                        validation.set(None);
                        failure.set(None);
                        submitting.set(true);
                        let lot_id = lot_id.clone();
                        let (amount, authorization_id, command_id) = match pending {
                            PendingProxyCommand::Lower { amount, authorization_id, command_id } => {
                                (amount, authorization_id, command_id)
                            }
                            PendingProxyCommand::Create { .. } => return,
                        };
                        spawn(async move {
                            let command_result =
                                lower_proxy_authorization(lot_id, authorization_id, amount, command_id)
                                    .await;
                            submitting.set(false);
                            match command_result {
                                Ok(authorization) => {
                                    command.set(None);
                                    saved_authorization.set(Some(Box::new(authorization)));
                                    refresh_generation.set(refresh_generation().saturating_add(1));
                                }
                                Err(error) => {
                                    confirmation.set(None);
                                    failure.set(Some(error));
                                    // 降低失败也要刷新权威 lot/代理状态，但不能清除可重试命令。
                                    refresh_generation.set(refresh_generation().saturating_add(1));
                                }
                            }
                        });
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
                            {t!("bid-minimum", price: format_amount(lower_bound))}
                        }
                        label { for: maximum_id.clone(), {t!("proxy-lower-label")} }
                        input {
                            id: maximum_id,
                            r#type: "text",
                            autofocus: true,
                            inputmode: "decimal",
                            aria_describedby: "public-proxy-lower-hint",
                            disabled: submitting(),
                            value: "{maximum_amount}",
                            oninput: move |event| maximum_amount.set(event.value()),
                        }
                        {amount_hint(
                            &maximum_amount(),
                            "public-proxy-lower-hint",
                            "proxy-invalid-amount",
                        )}
                        p { class: "public-auction-bid-form__hint", {t!("proxy-shorthand-hint")} }
                    }
                    if let Some(message) = validation() {
                        p { class: "action-feedback action-feedback--error", role: "alert", "{message}" }
                    }
                    if let Some(error) = failure() {
                        p { class: "action-feedback action-feedback--error", role: "alert",
                            "{proxy_error_message(&error, lower_bound)}"
                        }
                    }
                    div { class: "public-auction-bid-form__actions",
                        button {
                            r#type: "button",
                            disabled: submitting(),
                            onclick: move |_| {
                                if confirmation().is_some() {
                                    confirmation.set(None);
                                } else {
                                    on_close.call(());
                                }
                            },
                            if confirming { {t!("proxy-confirm-back")} } else { {t!("dialog-cancel")} }
                        }
                        button {
                            r#type: "submit",
                            disabled: submitting(),
                            aria_describedby: if confirming { confirmation_id.clone() } else { String::new() },
                            if submitting() {
                                {t!("proxy-working")}
                            } else if confirming {
                                {t!("proxy-lower-confirm-submit")}
                            } else {
                                {t!("proxy-lower-submit")}
                            }
                        }
                    }
                }
            } else {
                p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
            }
        }
    }
}
