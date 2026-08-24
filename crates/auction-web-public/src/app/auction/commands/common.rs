//! 竞价命令组件共用的短生命周期状态和安全错误映射。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::CommandError;

use super::super::{format_amount, parse_credit_amount};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingBidCommand {
    pub(super) amount: i64,
    pub(super) command_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PendingProxyCommand {
    Create { amount: i64, mode: String, command_id: Uuid },
    Lower { amount: i64, authorization_id: String, command_id: Uuid },
}

pub(super) fn amount_hint(value: &str, id: &'static str, invalid_key: &'static str) -> Element {
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

pub(super) fn bid_error_message(error: &CommandError, minimum: i64) -> String {
    match error.code.as_str() {
        "invalid_amount" => t!("bid-invalid-amount"),
        "bid_too_low" => t!("bid-below-minimum", price: format_amount(minimum)),
        "auction_not_open" => t!("bid-unavailable"),
        "command_conflict" => t!("command-conflict"),
        _ if error.status == 401 => t!("command-session-expired"),
        _ => t!("bid-error"),
    }
}

pub(super) fn proxy_error_message(error: &CommandError, minimum: i64) -> String {
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
