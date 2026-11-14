//! 结果不确定命令的原样重试；只重放已提交负载，不重新应用新命令的价格/能力校验。

use auction_web_shared::t;
use dioxus::prelude::*;

use super::super::format_credit_amount;
use super::bid::BidSuccess;
use super::pending::{
    BidConfirmCommand, PendingProxyCommand, ProxyConfirmCommand, bid_error_message,
    proxy_error_message,
};
use crate::api::{
    CommandError, ProxyCommandAuthorization, create_proxy_authorization, lower_proxy_authorization,
    place_bid,
};

/// 只有同一 lot/用户宿主保留的已提交命令可以进入此视图；输入只读，取消后重新开始。
#[component]
pub(crate) fn BidRetry(
    lot_id: String,
    lot_code: String,
    mut command: BidConfirmCommand,
    on_success: EventHandler<BidSuccess>,
    on_close: EventHandler<()>,
) -> Element {
    let mut refresh = use_context::<Signal<u64>>();
    let Some(pending) = (command.command)() else { return rsx! {} };
    let error = (command.failure)()
        .map(|error| retry_error_message(&error, |error| bid_error_message(error, pending.amount)));
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
                        place_bid(pending.user_id, lot_id, pending.amount, pending.command_id).await
                    },
                    CommandError::outcome_uncertain,
                    move |result| {
                        // 与普通出价表单一致：先重读权威投影，再由宿主关闭弹窗并提示结果。
                        refresh.set(refresh().saturating_add(1));
                        on_success.call(BidSuccess {
                            leading: result.leading,
                            current_price: result.current_price,
                        });
                    },
                    move || refresh.set(refresh().saturating_add(1)),
                );
            },
            p { {t!("command-retry-unchanged")} }
            p { {t!("bid-confirm-description", amount: format_credit_amount(pending.amount), code: lot_code)} }
            if let Some(error) = error {
                p { class: "action-feedback action-feedback--error", role: "alert", "{error}" }
            }
            div { class: "public-auction-bid-form__actions",
                button { r#type: "button", disabled: (command.submitting)(),
                    onclick: move |_| on_close.call(()), {t!("dialog-cancel")} }
                button {
                    r#type: "submit",
                    disabled: (command.submitting)(),
                    // 提交期间确认按钮被禁用，焦点已退到弹窗面板；重试视图挂载时把焦点交给
                    // 唯一的主操作，键盘用户不必再从关闭按钮一路 Tab 过来。
                    onmounted: move |event| {
                        let button = event.data();
                        async move {
                            let _ = button.set_focus(true).await;
                        }
                    },
                    if (command.submitting)() { {t!("bid-working")} } else { {t!("data-retry")} }
                }
            }
        }
    }
}

/// 原样重试被拒绝时的文案。重试按服务端当时的价格重新判定，`bid_too_low` 只说明价格已变化：
/// 本次提交的金额不是最低价，不能代入“至少为”文案，改为提示取消后按当前价格重新操作。
/// 其余错误码沿用新命令的映射（`command_message` 的金额参数只影响 `bid_too_low`）。
fn retry_error_message(
    error: &CommandError,
    command_message: impl FnOnce(&CommandError) -> String,
) -> String {
    if error.code == "bid_too_low" {
        return t!("command-retry-price-changed");
    }
    command_message(error)
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
                maximum: format_credit_amount(*amount),
                mode: if mode == "slow" { t!("proxy-mode-slow") } else { t!("proxy-mode-fast") }
            ),
        ),
        PendingProxyCommand::Lower { amount, .. } => (
            *amount,
            t!(
                "proxy-lower-confirm-description", maximum: format_credit_amount(*amount)
            ),
        ),
    };
    let error = (command.failure)()
        .map(|error| retry_error_message(&error, |error| proxy_error_message(error, amount)));
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
                            PendingProxyCommand::Create { user_id, amount, mode, command_id } => {
                                create_proxy_authorization(user_id, lot_id, amount, mode, command_id).await
                            }
                            PendingProxyCommand::Lower { user_id, amount, authorization_id, command_id } => {
                                lower_proxy_authorization(user_id, lot_id, authorization_id, amount, command_id).await
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
                button {
                    r#type: "submit",
                    disabled: (command.submitting)(),
                    // 提交期间确认按钮被禁用，焦点已退到弹窗面板；重试视图挂载时把焦点交给
                    // 唯一的主操作，键盘用户不必再从关闭按钮一路 Tab 过来。
                    onmounted: move |event| {
                        let button = event.data();
                        async move {
                            let _ = button.set_focus(true).await;
                        }
                    },
                    if (command.submitting)() { {t!("bid-working")} } else { {t!("data-retry")} }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use auction_web_shared::i18n::{DEFAULT_PUBLIC_LOCALE, build_bundle};
    use dioxus::prelude::*;
    use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;

    use super::{
        CommandError, bid_error_message, format_credit_amount, proxy_error_message,
        retry_error_message,
    };
    use crate::surface::router::PUBLIC_LOCALE_CATALOG;

    /// 用户在结果未知前提交的金额。
    const SUBMITTED: i64 = 50_000;

    thread_local! {
        static MESSAGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn error(code: &str) -> CommandError {
        CommandError { code: code.to_owned(), status: 409 }
    }

    /// 在挂载英文译文的组件里生成重试文案：出价被拒、代理被拒、出价的其他错误。
    #[component]
    fn RetryMessages() -> Element {
        let bundle = try_use_init_i18n_from_bundle(|| {
            build_bundle(
                &DEFAULT_PUBLIC_LOCALE,
                PUBLIC_LOCALE_CATALOG.default_ftl,
                &DEFAULT_PUBLIC_LOCALE,
                None,
            )
        });
        if bundle.is_ok() {
            let too_low = error("bid_too_low");
            let leading = error("already_leading");
            let messages = vec![
                retry_error_message(&too_low, |error| bid_error_message(error, SUBMITTED)),
                retry_error_message(&too_low, |error| proxy_error_message(error, SUBMITTED)),
                retry_error_message(&leading, |error| bid_error_message(error, SUBMITTED)),
                bid_error_message(&leading, SUBMITTED),
            ];
            MESSAGES.with(|rendered| *rendered.borrow_mut() = messages);
        }
        rsx! {}
    }

    #[test]
    fn price_change_on_retry_does_not_quote_the_submitted_amount_as_minimum() {
        let mut dom = VirtualDom::new(RetryMessages);
        dom.rebuild_in_place();
        let messages = MESSAGES.with(RefCell::take);
        assert_eq!(messages.len(), 4, "译文未能初始化");
        let submitted = format_credit_amount(SUBMITTED);
        for message in &messages[..2] {
            assert!(!message.contains(&submitted), "{message}");
            assert!(message.contains("price has changed"), "{message}");
        }
        // 其他错误码与新命令的映射一致。
        assert_eq!(messages[2], messages[3]);
    }
}
