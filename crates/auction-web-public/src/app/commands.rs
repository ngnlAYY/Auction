//! Public 竞价命令组件。
//!
//! 组件只负责收集用户金额、显示命令状态和投影服务端确认；金额步进、竞价资格与状态版本
//! 不在浏览器端复制。一次输入在失败重试时复用同一个 `command_id`，保证服务端幂等账本能够
//! 将网络重试识别为同一条命令。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    CommandError, OwnedProxy, ProxyCommandAuthorization, create_proxy_authorization,
    lower_proxy_authorization, place_bid,
};

use super::{
    RetryPlaceholder, format_amount, parse_credit_amount,
    proxy::{proxy_status_label, proxy_transition_label},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingBidCommand {
    amount: i64,
    command_id: Uuid,
}

/// 普通竞价表单参数；关闭与成功动作分开表达，避免调用方误用弹窗生命周期。
pub(super) struct BidFormInput {
    pub lot_id: String,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub can_bid: bool,
    pub blocked: bool,
    pub blocked_reason: String,
    pub on_accepted: EventHandler<()>,
    pub on_close: EventHandler<()>,
}

/// 显示普通竞价表单；最低价和 lot 状态只用于即时反馈，最终结果由服务端决定。
pub(super) fn bid_form(input: BidFormInput) -> Element {
    let BidFormInput {
        lot_id,
        current_price,
        next_bid,
        lot_status,
        can_bid,
        blocked,
        blocked_reason,
        on_accepted,
        on_close,
    } = input;
    let amount_id = format!("public-bid-amount-{lot_id}");
    let mut amount = use_signal(|| format_amount(next_bid));
    let mut failure = use_signal(|| None::<CommandError>);
    let mut validation = use_signal(|| None::<String>);
    let mut submitting = use_signal(|| false);
    let mut command = use_signal(|| None::<PendingBidCommand>);
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = can_bid && lot_status == "live";
    let blocked = blocked || !bidding_open;
    let blocked_reason = if bidding_open { blocked_reason } else { t!("bid-unavailable") };

    rsx! {
        form {
            class: "public-auction-bid-form",
            aria_busy: submitting().to_string(),
            onsubmit: move |event| {
                event.prevent_default();
                if submitting() || blocked {
                    return;
                }
                let Some(value) = parse_credit_amount(&amount()) else {
                    validation.set(Some(t!("bid-invalid-amount")));
                    failure.set(None);
                    return;
                };
                if value < next_bid {
                    validation.set(Some(t!("bid-below-minimum", price: format_amount(next_bid))));
                    failure.set(None);
                    return;
                }
                let pending = match command() {
                    Some(existing) if existing.amount == value => existing,
                    _ => PendingBidCommand { amount: value, command_id: Uuid::new_v4() },
                };
                command.set(Some(pending));
                validation.set(None);
                failure.set(None);
                submitting.set(true);
                let lot_id = lot_id.clone();
                spawn(async move {
                    let bid_result = place_bid(lot_id, pending.amount, pending.command_id).await;
                    submitting.set(false);
                    match bid_result {
                        Ok(_) => {
                            command.set(None);
                            refresh_generation.set(refresh_generation().saturating_add(1));
                            on_accepted.call(());
                        }
                        Err(error) => failure.set(Some(error)),
                    }
                });
            },
            p { class: "public-auction-bid-form__meta",
                {t!("bid-current-price", price: format_amount(current_price))}
                " · "
                {t!("bid-minimum", price: format_amount(next_bid))}
            }
            label { for: amount_id.clone(), {t!("bid-amount-label")} }
            input {
                id: amount_id,
                r#type: "text",
                autofocus: true,
                inputmode: "decimal",
                aria_describedby: "public-bid-amount-hint",
                disabled: submitting() || blocked,
                value: "{amount}",
                oninput: move |event| amount.set(event.value()),
            }
            {amount_hint(&amount(), "public-bid-amount-hint", "bid-invalid-amount")}
            p { class: "public-auction-bid-form__hint", {t!("bid-shorthand-hint")} }
            if blocked {
                p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
            }
            if let Some(message) = validation() {
                p { class: "action-feedback action-feedback--error", role: "alert", "{message}" }
            }
            if let Some(error) = failure() {
                p { class: "action-feedback action-feedback--error", role: "alert",
                    "{bid_error_message(&error, next_bid)}"
                }
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {t!("dialog-cancel")}
                }
                button { r#type: "submit", disabled: submitting() || blocked,
                    if submitting() { {t!("bid-working")} } else { {t!("bid-submit")} }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PendingProxyCommand {
    Create { amount: i64, mode: String, command_id: Uuid },
    Lower { amount: i64, authorization_id: String, command_id: Uuid },
}

/// 公开 lot 的代理授权表单参数；账户状态由服务端投影，不在浏览器端复制代理状态机。
pub(super) struct ProxyFormInput {
    pub lot_id: String,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub allow_slow: bool,
    pub leading: bool,
    pub blocked_reason: String,
    pub proxy: Option<Result<Option<OwnedProxy>, String>>,
    pub refresh_generation: Signal<u64>,
    pub saved_authorization: Signal<Option<Box<ProxyCommandAuthorization>>>,
    pub on_close: EventHandler<()>,
}

/// 公开 lot 的代理授权入口。
pub(super) fn proxy_form(input: ProxyFormInput) -> Element {
    let ProxyFormInput {
        lot_id,
        current_price,
        next_bid,
        lot_status,
        allow_slow,
        leading,
        blocked_reason,
        proxy,
        refresh_generation,
        saved_authorization,
        on_close,
    } = input;
    if let Some(authorization) = saved_authorization() {
        return proxy_success_panel(authorization, on_close);
    }
    match proxy.as_ref() {
        None => rsx! { p { role: "status", {t!("proxy-loading")} } },
        Some(Err(_)) => rsx! {
            RetryPlaceholder {
                title: t!("catalog-proxy"),
                detail: t!("proxy-load-error"),
                refresh_generation,
            }
        },
        Some(Ok(None)) => rsx! {
            ProxyCreateForm {
                lot_id,
                current_price,
                next_bid,
                initial_minimum: next_bid,
                lot_status,
                allow_slow,
                blocked: leading,
                blocked_reason,
                saved_authorization,
                on_close,
            }
        },
        Some(Ok(Some(proxy))) if proxy.status == "active" => rsx! {
            ProxyAuthorizationForm {
                lot_id,
                current_price,
                next_bid,
                lot_status,
                proxy: proxy.clone(),
                saved_authorization,
                on_close,
            }
        },
        Some(Ok(Some(proxy))) => rsx! {
            ProxyCreateForm {
                lot_id,
                current_price,
                next_bid,
                initial_minimum: next_bid.max(proxy.maximum_amount.saturating_add(1)),
                lot_status,
                allow_slow,
                blocked: leading,
                blocked_reason,
                saved_authorization,
                on_close,
            }
        },
    }
}

#[component]
fn ProxyCreateForm(
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
                            Err(error) => failure.set(Some(error)),
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

#[component]
fn ProxyAuthorizationForm(
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
    let mut refresh_generation = use_context::<Signal<u64>>();
    let bidding_open = lot_status == "live";
    let can_lower = proxy.capabilities.can_lower && bidding_open;
    let blocked_reason =
        if !bidding_open { t!("bid-unavailable") } else { t!("proxy-cannot-lower") };

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
                            validation.set(Some(t!("proxy-below-minimum", price: format_amount(lower_bound))));
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
                                PendingProxyCommand::Lower { amount, authorization_id, command_id }
                            }
                            _ => PendingProxyCommand::Lower {
                                amount: value,
                                authorization_id,
                                command_id: Uuid::new_v4(),
                            },
                        };
                        command.set(Some(pending.clone()));
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
                                Err(error) => failure.set(Some(error)),
                            }
                        });
                    },
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
                            onclick: move |_| on_close.call(()),
                            {t!("dialog-cancel")}
                        }
                        button { r#type: "submit", disabled: submitting(),
                            if submitting() { {t!("proxy-working")} } else { {t!("proxy-lower-submit")} }
                        }
                    }
                }
            } else {
                p { class: "action-feedback action-feedback--notice", role: "note", "{blocked_reason}" }
            }
        }
    }
}

fn proxy_success_panel(
    authorization: Box<ProxyCommandAuthorization>,
    on_close: EventHandler<()>,
) -> Element {
    let success_message = match authorization.bid.as_ref() {
        Some(bid) if bid.leading => {
            t!("proxy-leading-now", price: format_amount(bid.current_price))
        }
        _ => t!("proxy-saved"),
    };
    let quota = if authorization.quota.limit == 0 {
        t!("account-quota-unlimited", used: authorization.quota.used)
    } else {
        t!(
            "account-quota-bounded",
            used: authorization.quota.used,
            limit: authorization.quota.limit,
            remaining: authorization.quota.limit.saturating_sub(authorization.quota.used)
        )
    };
    let transitions = if authorization.transitions.is_empty() {
        t!("value-empty")
    } else {
        authorization
            .transitions
            .iter()
            .map(|transition| {
                format!(
                    "{}: {} · {}",
                    transition.transition_number,
                    proxy_transition_label(&transition.kind),
                    format_amount(transition.new_maximum_amount),
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let status = proxy_status_label(&authorization.status);

    rsx! {
        section { class: "public-auction-bid-form",
            p { class: "action-feedback action-feedback--success", role: "status",
                "{success_message}"
            }
            p { class: "public-auction-bid-form__meta",
                {t!("proxy-status", status: status)}
                " · "
                "{quota}"
            }
            p { class: "public-auction-bid-form__meta public-auction-bid-form__history",
                {t!("account-transition-history")}
                ": "
                "{transitions}"
            }
            div { class: "public-auction-bid-form__actions",
                button {
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    {t!("catalog-close")}
                }
            }
        }
    }
}

fn amount_hint(value: &str, id: &'static str, invalid_key: &'static str) -> Element {
    if value.trim().is_empty() {
        return rsx! {};
    }
    match parse_credit_amount(value) {
        Some(amount) => rsx! {
            p {
                id,
                class: "public-auction-bid-form__amount-hint",
                role: "status",
                "= {format_amount(amount)}"
            }
        },
        None => rsx! {
            p {
                id,
                class: "public-auction-bid-form__amount-hint public-auction-bid-form__amount-hint--invalid",
                role: "alert",
                {t!(invalid_key)}
            }
        },
    }
}

fn bid_error_message(error: &CommandError, minimum: i64) -> String {
    match error.code.as_str() {
        "invalid_amount" => t!("bid-invalid-amount"),
        "bid_too_low" => t!("bid-below-minimum", price: format_amount(minimum)),
        "auction_not_open" => t!("bid-unavailable"),
        "command_conflict" => t!("command-conflict"),
        _ if error.status == 401 => t!("command-session-expired"),
        _ => t!("bid-error"),
    }
}

fn proxy_error_message(error: &CommandError, minimum: i64) -> String {
    match error.code.as_str() {
        "invalid_amount" => t!("proxy-invalid-amount"),
        "bid_too_low" => t!("proxy-below-minimum", price: format_amount(minimum)),
        "invalid_proxy_maximum" => t!("proxy-not-lower"),
        "invalid_proxy_state" => t!("proxy-cannot-lower"),
        "auction_not_open" => t!("bid-unavailable"),
        "command_conflict" => t!("command-conflict"),
        _ if error.status == 401 => t!("command-session-expired"),
        _ => t!("proxy-command-error"),
    }
}
