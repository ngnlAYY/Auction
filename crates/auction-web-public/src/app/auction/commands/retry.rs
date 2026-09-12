//! 结果不确定命令的原样重试；只重放已提交负载，不重新应用新命令的价格/能力校验。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    CommandError, ProxyCommandAuthorization, create_proxy_authorization, lower_proxy_authorization,
    place_bid,
};

use super::super::format_amount;
use super::bid::BidSuccess;
use super::common::{
    BidConfirmCommand, PendingProxyCommand, ProxyConfirmCommand, bid_error_message,
    proxy_error_message,
};

/// 只有同一 lot/用户宿主保留的已提交命令可以进入此视图；输入只读，取消后重新开始。
#[component]
pub(crate) fn BidRetry(
    lot_id: String,
    lot_code: String,
    mut command: BidConfirmCommand,
    mut success: Signal<Option<BidSuccess>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut refresh = use_context::<Signal<u64>>();
    let Some(pending) = (command.command)() else { return rsx! {} };
    let error = (command.failure)().map(|error| bid_error_message(&error, pending.amount));
    rsx! {
        form {
            class: "public-auction-bid-form",
            aria_busy: (command.submitting)().to_string(),
            onsubmit: move |event| {
                event.prevent_default();
                if (command.submitting)() || !(command.uncertain)() {
                    return;
                }
                let lot_id = lot_id.clone();
                command.submit(
                    pending,
                    move |pending| async move {
                        place_bid(lot_id, pending.amount, pending.command_id).await
                    },
                    CommandError::outcome_uncertain,
                    move |result| {
                        success.set(Some(BidSuccess {
                            leading: result.leading,
                            current_price: result.current_price,
                        }));
                        refresh.set(refresh().saturating_add(1));
                    },
                    move || refresh.set(refresh().saturating_add(1)),
                );
            },
            p { {t!("command-retry-unchanged")} }
            p { {t!("bid-confirm-description", amount: format_amount(pending.amount), code: lot_code)} }
            if let Some(error) = error {
                p { class: "action-feedback action-feedback--error", role: "alert", "{error}" }
            }
            div { class: "public-auction-bid-form__actions",
                button { r#type: "button", disabled: (command.submitting)(),
                    onclick: move |_| on_close.call(()), {t!("dialog-cancel")} }
                button { r#type: "submit", disabled: (command.submitting)(),
                    if (command.submitting)() { {t!("bid-working")} } else { {t!("data-retry")} }
                }
            }
        }
    }
}

/// 不依赖刷新后的活动授权形状，创建已生效或上限已降低时仍可读取原命令结果。
#[component]
pub(super) fn ProxyRetry(
    lot_id: String,
    mut command: ProxyConfirmCommand,
    mut saved_authorization: Signal<Option<Box<ProxyCommandAuthorization>>>,
    on_close: EventHandler<()>,
) -> Element {
    let mut refresh = use_context::<Signal<u64>>();
    let Some(pending) = (command.command)() else { return rsx! {} };
    let (amount, description) = match &pending {
        PendingProxyCommand::Create { amount, mode, .. } => (
            *amount,
            t!(
                "proxy-create-confirm-description",
                maximum: format_amount(*amount),
                mode: if mode == "slow" { t!("proxy-mode-slow") } else { t!("proxy-mode-fast") }
            ),
        ),
        PendingProxyCommand::Lower { amount, .. } => (
            *amount,
            t!(
                "proxy-lower-confirm-description", maximum: format_amount(*amount)
            ),
        ),
    };
    let error = (command.failure)().map(|error| proxy_error_message(&error, amount));
    rsx! {
        form {
            class: "public-auction-bid-form",
            aria_busy: (command.submitting)().to_string(),
            onsubmit: move |event| {
                event.prevent_default();
                if (command.submitting)() || !(command.uncertain)() {
                    return;
                }
                let lot_id = lot_id.clone();
                command.submit(
                    pending.clone(),
                    move |pending| async move {
                        match pending {
                            PendingProxyCommand::Create { amount, mode, command_id } => {
                                create_proxy_authorization(lot_id, amount, mode, command_id).await
                            }
                            PendingProxyCommand::Lower { amount, authorization_id, command_id } => {
                                lower_proxy_authorization(lot_id, authorization_id, amount, command_id).await
                            }
                        }
                    },
                    CommandError::outcome_uncertain,
                    move |authorization| {
                        saved_authorization.set(Some(Box::new(authorization)));
                        refresh.set(refresh().saturating_add(1));
                    },
                    move || refresh.set(refresh().saturating_add(1)),
                );
            },
            p { {t!("command-retry-unchanged")} }
            p { "{description}" }
            if let Some(error) = error {
                p { class: "action-feedback action-feedback--error", role: "alert", "{error}" }
            }
            div { class: "public-auction-bid-form__actions",
                button { r#type: "button", disabled: (command.submitting)(),
                    onclick: move |_| on_close.call(()), {t!("dialog-cancel")} }
                button { r#type: "submit", disabled: (command.submitting)(),
                    if (command.submitting)() { {t!("bid-working")} } else { {t!("data-retry")} }
                }
            }
        }
    }
}
