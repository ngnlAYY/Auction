//! 已有代理授权的降低上限表单及其提交状态。

use auction_contracts::MAX_PUBLIC_BID_AMOUNT;
use auction_web_shared::{format_credit_amount, t};
use dioxus::prelude::*;

use super::super::super::parse_credit_amount;
use super::super::super::proxy::proxy_status_label;
use super::super::host::CompetitionCommands;
use super::super::pending::{
    PendingProxyCommand, ProxyConfirmCommand, PublicCommandIdentity, amount_hint, confirm_panel,
    initial_proxy_amount,
};
use super::super::session::CommandSession;
use crate::api::OwnedProxy;

#[component]
pub(super) fn ProxyAuthorizationForm(
    lot_id: String,
    current_price: i64,
    next_bid: i64,
    lot_status: String,
    proxy: OwnedProxy,
    authorization_ready: bool,
    authorize: Callback<(), bool>,
    on_submit: EventHandler<PendingProxyCommand>,
    /// 由弹窗宿主注入的两阶段确认命令 hook（降低上限实例；见 ProxyFormInput）。
    command: ProxyConfirmCommand,
    on_close: EventHandler<()>,
) -> Element {
    let user_id = use_context::<PublicCommandIdentity>().0;
    let session = use_context::<CommandSession>();
    let can_submit = authorization_ready && session.can_submit_new();
    let commands = try_use_context::<CompetitionCommands>();
    let maximum_id = format!("public-proxy-lower-maximum-{lot_id}");
    let lower_bound = if proxy.capabilities.waiting { next_bid } else { current_price };
    let mut maximum_amount = use_signal(|| initial_proxy_amount(lower_bound));
    let mut validation = use_signal(|| None::<String>);
    let mut command = command;
    let bidding_open = lot_status == "live";
    let limit_reached = lower_bound > MAX_PUBLIC_BID_AMOUNT;
    let can_lower = proxy.capabilities.can_lower && bidding_open && !limit_reached;
    let blocked_reason = if !bidding_open {
        t!("bid-unavailable")
    } else if limit_reached {
        t!("bid-limit-note")
    } else {
        t!("proxy-cannot-lower")
    };
    let confirmation_id = format!("public-proxy-lower-confirmation-{lot_id}");
    let confirmation = match (command.confirmation)() {
        Some(PendingProxyCommand::Lower { amount, .. }) => Some(amount),
        _ => None,
    };
    let confirming = confirmation.is_some();

    let submit = use_callback(move |()| {
        if !can_lower
            || !authorize.call(())
            || !session.can_submit_new()
            || !commands.is_some_and(|commands| commands.can_start(&lot_id))
        {
            return;
        }
        // 两阶段确认：签名取 (amount, authorization_id)，同输入重试复用同一
        // command_id（幂等）。
        let pending = if let Some(pending) = (command.confirmation)() {
            match &pending {
                PendingProxyCommand::Lower { amount, authorization_id, .. } => {
                    if *amount > MAX_PUBLIC_BID_AMOUNT {
                        command.cancel_confirmation();
                        validation.set(Some(t!("proxy-invalid-amount")));
                        return;
                    }
                    if authorization_id != &proxy.authorization_id {
                        // 授权已变更：整条命令作废，彻底清空后退回编辑态。
                        command.reset();
                        validation.set(Some(t!("proxy-authorization-not-active")));
                        return;
                    }
                    if *amount >= proxy.maximum_amount {
                        command.cancel_confirmation();
                        validation.set(Some(t!("proxy-not-lower")));
                        return;
                    }
                    if *amount < lower_bound {
                        command.cancel_confirmation();
                        validation.set(Some(t!(
                            "proxy-below-minimum",
                            price: format_credit_amount(lower_bound)
                        )));
                        return;
                    }
                }
                // 本表单只冻结 Lower；防御性处理串型，退回编辑态。
                PendingProxyCommand::Create { .. } => {
                    command.reset();
                    return;
                }
            }
            pending
        } else {
            let Some(value) = parse_credit_amount(&maximum_amount()) else {
                validation.set(Some(t!("proxy-invalid-amount")));
                return;
            };
            if value >= proxy.maximum_amount {
                validation.set(Some(t!("proxy-not-lower")));
                return;
            }
            if value < lower_bound {
                validation.set(Some(t!(
                    "proxy-below-minimum",
                    price: format_credit_amount(lower_bound)
                )));
                return;
            }
            let authorization_id = proxy.authorization_id.clone();
            command.begin(format!("{value}:{authorization_id}"), |command_id| {
                PendingProxyCommand::Lower { user_id, amount: value, authorization_id, command_id }
            });
            validation.set(None);
            return;
        };
        validation.set(None);
        on_submit.call(pending);
    });
    rsx! {
        section {
            class: "public-auction-bid-form",
            h2 { {t!("proxy-current")} }
            p { {t!("proxy-status", status: proxy_status_label(&proxy.status))} }
            p { {t!("proxy-maximum", maximum: format_credit_amount(proxy.maximum_amount))} }
            if can_lower {
                form {
                    onsubmit: move |event| {
                        event.prevent_default();
                        submit.call(());
                    },
                    if let Some(amount) = confirmation {
                        {confirm_panel(
                            &confirmation_id,
                            &t!("bid-type-proxy-lower"),
                            format_credit_amount(amount),
                            t!("proxy-binding-note"),
                        )}
                    } else {
                        p { class: "public-auction-bid-form__meta",
                            {t!("bid-minimum", price: format_credit_amount(lower_bound))}
                        }
                        label { for: maximum_id.clone(), {t!("proxy-lower-label")} }
                        input {
                            id: maximum_id,
                            name: "maximum_amount",
                            r#type: "text",
                            autofocus: true,
                            // 与创建代理一致，允许在手机键盘输入 K/M 缩写。
                            inputmode: "text",
                            enterkeyhint: "done",
                            autocapitalize: "none",
                            autocomplete: "off",
                            spellcheck: "false",
                            aria_invalid: validation().is_some().to_string(),
                            aria_describedby: if validation().is_some() {
                                "public-proxy-lower-hint public-proxy-lower-error"
                            } else {
                                "public-proxy-lower-hint"
                            },
                            value: "{maximum_amount}",
                            // 与直接出价一致：从确认视图“返回修改”或提交被拒后上限框重新挂载，
                            // 此时焦点还留在按钮上，这里把它交还给要修改的上限。
                            onmounted: move |event| {
                                let input = event.data();
                                async move {
                                    let _ = input.set_focus(true).await;
                                }
                            },
                            oninput: move |event| {
                                maximum_amount.set(event.value());
                                validation.set(None);
                            },
                        }
                        {amount_hint(
                            &maximum_amount(),
                            "public-proxy-lower-hint",
                            "proxy-invalid-amount",
                        )}
                        p { class: "public-auction-bid-form__hint", {t!("proxy-shorthand-hint")} }
                    }
                    if let Some(message) = validation() {
                        p { id: "public-proxy-lower-error", class: "action-feedback action-feedback--error", role: "alert", "{message}" }
                    }
                    div { class: "public-auction-bid-form__actions",
                        button {
                            r#type: "button",
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
                            // 确认视图的首选焦点是确认按钮，而不是弹窗右上角的关闭按钮。
                            autofocus: confirming,
                            disabled: !can_submit,
                            aria_describedby: if confirming { confirmation_id } else { String::new() },
                            if confirming {
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
