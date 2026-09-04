//! 竞价命令组件共用的短生命周期状态和安全错误映射。

use auction_web_shared::{ConfirmCommand, legacy_t as t};
use dioxus::prelude::*;
use uuid::Uuid;

use crate::api::CommandError;

use super::super::{format_amount, parse_credit_amount};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingBidCommand {
    pub(super) amount: i64,
    pub(super) command_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PendingProxyCommand {
    Create { amount: i64, mode: String, command_id: Uuid },
    Lower { amount: i64, authorization_id: String, command_id: Uuid },
}

/// 普通竞价的两阶段确认命令 hook。由弹窗宿主持有（整体 Copy），表单经 prop 共享，
/// 宿主据此接线 Dialog 的 busy 关闭守卫。
///
/// 包一层新类型只为实现 `PartialEq`（`#[component]` props 要求）：逐字段比较 Signal
/// 身份（Signal 的 PartialEq 是指针比较，不读值、不触发订阅），同一 hook 实例相等。
/// `Deref`/`DerefMut` 透传内部 hook，调用点与直接使用 `ConfirmCommand` 一致。
#[derive(Clone, Copy)]
pub(crate) struct BidConfirmCommand(pub ConfirmCommand<PendingBidCommand, CommandError>);

impl PartialEq for BidConfirmCommand {
    fn eq(&self, other: &Self) -> bool {
        self.0.command == other.0.command
            && self.0.confirmation == other.0.confirmation
            && self.0.submitting == other.0.submitting
            && self.0.failure == other.0.failure
    }
}

impl std::ops::Deref for BidConfirmCommand {
    type Target = ConfirmCommand<PendingBidCommand, CommandError>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for BidConfirmCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// 代理授权（创建/降低上限各持有一份实例）的两阶段确认命令 hook；包装语义与
/// [`BidConfirmCommand`] 相同。
#[derive(Clone, Copy)]
pub(crate) struct ProxyConfirmCommand(pub ConfirmCommand<PendingProxyCommand, CommandError>);

impl PartialEq for ProxyConfirmCommand {
    fn eq(&self, other: &Self) -> bool {
        self.0.command == other.0.command
            && self.0.confirmation == other.0.confirmation
            && self.0.submitting == other.0.submitting
            && self.0.failure == other.0.failure
    }
}

impl std::ops::Deref for ProxyConfirmCommand {
    type Target = ConfirmCommand<PendingProxyCommand, CommandError>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ProxyConfirmCommand {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
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
        "unauthenticated" => t!("command-login-required"),
        "not_found" => t!("command-not-found"),
        "bidder_ineligible" => t!("bid-ineligible"),
        "already_leading" => t!("bid-already-leading"),
        "active_proxy_exists" => t!("proxy-active-exists"),
        "invalid_policy" => t!("proxy-invalid-policy"),
        "auction_not_open" => t!("bid-unavailable"),
        "command_conflict" => t!("command-conflict"),
        _ if error.status == 401 => t!("command-session-expired"),
        _ => t!("bid-error"),
    }
}

pub(super) fn proxy_error_message(error: &CommandError, minimum: i64) -> String {
    match error.code.as_str() {
        "invalid_amount" | "invalid_maximum_amount" => t!("proxy-invalid-amount"),
        "bid_too_low" => t!("proxy-below-minimum", price: format_amount(minimum)),
        "unauthenticated" => t!("command-login-required"),
        "not_found" => t!("command-not-found"),
        "bidder_ineligible" => t!("bid-ineligible"),
        "already_leading" => t!("bid-already-leading"),
        "active_proxy_exists" => t!("proxy-active-exists"),
        "proxy_authorization_not_active" => t!("proxy-authorization-not-active"),
        "proxy_maximum_not_lower" => t!("proxy-maximum-not-lower"),
        "proxy_maximum_below_binding" => t!("proxy-maximum-below-binding"),
        "proxy_reentry_maximum_not_higher" => t!("proxy-reentry-maximum-not-higher"),
        "proxy_outbid_limit" => t!("proxy-outbid-limit"),
        "proxy_disabled" => t!("proxy-disabled-error"),
        "proxy_slow_final" => t!("proxy-slow-final-error"),
        "invalid_policy" => t!("proxy-invalid-policy"),
        "auction_not_open" => t!("bid-unavailable"),
        "command_conflict" => t!("command-conflict"),
        _ if error.status == 401 => t!("command-session-expired"),
        _ => t!("proxy-command-error"),
    }
}
