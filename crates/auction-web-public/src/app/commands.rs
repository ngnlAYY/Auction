//! Public 竞价命令组件。
//!
//! 组件只负责收集用户金额、显示命令状态和投影服务端确认；金额步进、竞价资格与状态版本
//! 不在浏览器端复制。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{
    BidResult, OwnedProxy, ProxyCommandAuthorization, create_proxy_authorization, fetch_my_proxy,
    lower_proxy_authorization, place_bid,
};

#[derive(Debug, Clone, PartialEq)]
enum BidState {
    Accepted(BidResult),
    Failed,
}

pub(super) fn bid_form(lot_id: String) -> Element {
    let mut amount = use_signal(String::new);
    let mut result = use_signal(|| None::<BidState>);
    let mut submitting = use_signal(|| false);

    rsx! {
        form { class: "public-auction-bid-form",
            onsubmit: move |event| {
                event.prevent_default();
                if submitting() {
                    return;
                }
                let Ok(value) = amount().trim().parse::<i64>() else {
                    result.set(Some(BidState::Failed));
                    return;
                };
                if value <= 0 {
                    result.set(Some(BidState::Failed));
                    return;
                }
                submitting.set(true);
                let lot_id = lot_id.clone();
                spawn(async move {
                    let bid_result = place_bid(lot_id, value).await;
                    submitting.set(false);
                    result.set(Some(match bid_result {
                        Ok(bid) => BidState::Accepted(bid),
                        Err(_) => BidState::Failed,
                    }));
                });
            },
            label { for: "public-bid-amount", {t!("bid-amount-label")} }
            input {
                id: "public-bid-amount",
                r#type: "number",
                min: "1",
                value: "{amount}",
                oninput: move |event| amount.set(event.value()),
            }
            button { r#type: "submit", disabled: submitting(),
                if submitting() { {t!("bid-working")} } else { {t!("bid-submit")} }
            }
            if let Some(state) = result() {
                match state {
                    BidState::Accepted(bid) => rsx! {
                        p { role: "status",
                            {t!("bid-success", price: bid.current_price, version: bid.state_version)}
                        }
                    },
                    BidState::Failed => rsx! {
                        p { role: "alert", {t!("bid-error")} }
                    },
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ProxyCommandState {
    Accepted(Box<ProxyCommandAuthorization>),
    Failed,
}

/// 公开 lot 的代理授权入口；账户状态由服务端投影，不在浏览器端复制代理状态机。
pub(super) fn proxy_form(lot_id: String) -> Element {
    let requested_lot_id = lot_id.clone();
    let proxy = use_resource(move || {
        let lot_id = requested_lot_id.clone();
        async move { fetch_my_proxy(lot_id).await }
    });

    match &*proxy.read_unchecked() {
        None => rsx! { p { {t!("proxy-loading")} } },
        Some(Err(_)) => rsx! { p { role: "alert", {t!("proxy-load-error")} } },
        Some(Ok(None)) => rsx! { ProxyCreateForm { lot_id } },
        Some(Ok(Some(proxy))) => rsx! {
            ProxyAuthorizationForm { lot_id, proxy: proxy.clone() }
        },
    }
}

#[component]
fn ProxyCreateForm(lot_id: String) -> Element {
    let mut maximum_amount = use_signal(String::new);
    let mut mode = use_signal(|| "fast".to_owned());
    let mut result = use_signal(|| None::<ProxyCommandState>);
    let mut submitting = use_signal(|| false);

    rsx! {
        section { class: "public-auction-bid-form", aria_labelledby: "public-proxy-title",
            h2 { id: "public-proxy-title", {t!("proxy-title")} }
            form {
                onsubmit: move |event| {
                    event.prevent_default();
                    if submitting() {
                        return;
                    }
                    let Ok(value) = maximum_amount().trim().parse::<i64>() else {
                        result.set(Some(ProxyCommandState::Failed));
                        return;
                    };
                    if value <= 0 || !matches!(mode().as_str(), "fast" | "slow") {
                        result.set(Some(ProxyCommandState::Failed));
                        return;
                    }
                    submitting.set(true);
                    let lot_id = lot_id.clone();
                    let mode = mode();
                    spawn(async move {
                        let command_result =
                            create_proxy_authorization(lot_id, value, mode).await;
                        submitting.set(false);
                        result.set(Some(match command_result {
                            Ok(authorization) => {
                                ProxyCommandState::Accepted(Box::new(authorization))
                            }
                            Err(_) => ProxyCommandState::Failed,
                        }));
                    });
                },
                label { for: "public-proxy-maximum", {t!("proxy-create-maximum-label")} }
                input {
                    id: "public-proxy-maximum",
                    r#type: "number",
                    min: "1",
                    value: "{maximum_amount}",
                    oninput: move |event| maximum_amount.set(event.value()),
                }
                label { for: "public-proxy-mode", {t!("proxy-create-mode-label")} }
                select {
                    id: "public-proxy-mode",
                    value: "{mode}",
                    onchange: move |event| mode.set(event.value()),
                    option { value: "fast", {t!("proxy-mode-fast")} }
                    option { value: "slow", {t!("proxy-mode-slow")} }
                }
                button { r#type: "submit", disabled: submitting(),
                    if submitting() { {t!("proxy-working")} } else { {t!("proxy-create-submit")} }
                }
            }
            {proxy_command_feedback(result(), "proxy-create-success")}
        }
    }
}

#[component]
fn ProxyAuthorizationForm(lot_id: String, proxy: OwnedProxy) -> Element {
    let mut maximum_amount = use_signal(|| proxy.maximum_amount.to_string());
    let mut result = use_signal(|| None::<ProxyCommandState>);
    let mut submitting = use_signal(|| false);
    let authorization_id = proxy.authorization_id.clone();
    let can_lower = proxy.capabilities.can_lower;

    rsx! {
        section { class: "public-auction-bid-form", aria_labelledby: "public-proxy-title",
            h2 { id: "public-proxy-title", {t!("proxy-current")} }
            p { {t!("proxy-status", status: proxy.status.clone())} }
            p { {t!("proxy-maximum", maximum: proxy.maximum_amount)} }
            if can_lower {
                form {
                    onsubmit: move |event| {
                        event.prevent_default();
                        if submitting() {
                            return;
                        }
                        let Ok(value) = maximum_amount().trim().parse::<i64>() else {
                            result.set(Some(ProxyCommandState::Failed));
                            return;
                        };
                        if value <= 0 || value >= proxy.maximum_amount {
                            result.set(Some(ProxyCommandState::Failed));
                            return;
                        }
                        submitting.set(true);
                        let lot_id = lot_id.clone();
                        let authorization_id = authorization_id.clone();
                        spawn(async move {
                            let command_result =
                                lower_proxy_authorization(lot_id, authorization_id, value).await;
                            submitting.set(false);
                            result.set(Some(match command_result {
                                Ok(authorization) => {
                                    ProxyCommandState::Accepted(Box::new(authorization))
                                }
                                Err(_) => ProxyCommandState::Failed,
                            }));
                        });
                    },
                    label { for: "public-proxy-lower-maximum", {t!("proxy-lower-label")} }
                    input {
                        id: "public-proxy-lower-maximum",
                        r#type: "number",
                        min: "1",
                        value: "{maximum_amount}",
                        oninput: move |event| maximum_amount.set(event.value()),
                    }
                    button { r#type: "submit", disabled: submitting(),
                        if submitting() { {t!("proxy-working")} } else { {t!("proxy-lower-submit")} }
                    }
                }
                {proxy_command_feedback(result(), "proxy-lower-success")}
            }
        }
    }
}

fn proxy_command_feedback(state: Option<ProxyCommandState>, success_key: &'static str) -> Element {
    match state {
        Some(ProxyCommandState::Accepted(authorization)) => rsx! {
            p { role: "status",
                {t!(success_key,
                    maximum: authorization.maximum_amount,
                    version: authorization.state_version
                )}
            }
        },
        Some(ProxyCommandState::Failed) => rsx! {
            p { role: "alert", {t!("proxy-command-error")} }
        },
        None => rsx! {},
    }
}
