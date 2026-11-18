//! Public 代理命令组件的稳定出口。
//! 根据权威授权快照选择创建或降低上限的确认表单。

mod authorization;
mod create;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::pending::{PendingProxyCommand, ProxyConfirmCommand};
use super::session::CommandSession;
use crate::api::OwnedProxy;
use authorization::ProxyAuthorizationForm;
use create::ProxyCreateForm;

/// 公开 lot 的代理授权表单参数；账户状态由服务端投影，不在浏览器端复制代理状态机。
#[derive(Clone, PartialEq)]
pub struct ProxyFormInput {
    pub lot_id: String,
    pub can_bid: bool,
    pub current_price: i64,
    pub next_bid: i64,
    pub lot_status: String,
    pub allow_slow: bool,
    /// 该场冻结策略的代理默认模式，决定新建表单的初始选项；未知时新建表单失败关闭。
    pub default_mode: Option<String>,
    pub leading: bool,
    pub blocked_reason: String,
    pub proxy: Option<Result<Option<OwnedProxy>, String>>,
    pub refresh_generation: Signal<u64>,
    pub on_submit: EventHandler<PendingProxyCommand>,
    pub create_command: ProxyConfirmCommand,
    pub lower_command: ProxyConfirmCommand,
    pub on_close: EventHandler<()>,
}

/// 公开 lot 的代理授权入口。
#[component]
pub fn ProxyForm(input: ProxyFormInput) -> Element {
    let ProxyFormInput {
        lot_id,
        can_bid,
        current_price,
        next_bid,
        lot_status,
        allow_slow,
        default_mode,
        leading,
        blocked_reason,
        proxy,
        mut refresh_generation,
        on_submit,
        create_command,
        lower_command,
        on_close,
    } = input;
    let session = use_context::<CommandSession>();
    let last_success = use_hook(|| Rc::new(RefCell::new(None::<Option<OwnedProxy>>)));
    let mut last_success = last_success.borrow_mut();
    if !session.identity_confirmed() {
        *last_success = None;
        return rsx! { p { role: "status", {t!("bid-unavailable")} } };
    }
    // 保留同一宿主的最后成功投影来维持草稿；读取失败或等待重试时，旧投影不授权新写入。
    if let Some(Ok(snapshot)) = proxy.as_ref()
        && last_success.as_ref() != Some(snapshot)
    {
        *last_success = Some(snapshot.clone());
    }
    let authorization_ready = can_bid && proxy.as_ref().is_some_and(Result::is_ok);
    let form = match last_success.as_ref() {
        Some(Some(proxy)) if proxy.status == "active" => rsx! {
            ProxyAuthorizationForm {
                lot_id,
                current_price,
                next_bid,
                lot_status,
                proxy: proxy.clone(),
                authorization_ready,
                on_submit,
                command: lower_command,
                on_close,
            }
        },
        Some(snapshot) => rsx! {
            ProxyCreateForm {
                lot_id,
                initial_minimum: snapshot.as_ref().map_or(next_bid, |proxy| next_bid.max(proxy.maximum_amount.saturating_add(1))),
                lot_status,
                allow_slow,
                default_mode,
                blocked: leading,
                blocked_reason,
                authorization_ready,
                on_submit,
                command: create_command,
                on_close,
            }
        },
        None => rsx! {},
    };
    rsx! {
        {form}
        if !can_bid {
            p { role: "status", {t!("bid-unavailable")} }
        }
        if proxy.as_ref().is_some_and(Result::is_err) {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("catalog-proxy"),
                detail: t!("proxy-load-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
            }
        } else if can_bid && proxy.is_none() {
            p { role: "status", {t!("proxy-loading")} }
        }
    }
}
