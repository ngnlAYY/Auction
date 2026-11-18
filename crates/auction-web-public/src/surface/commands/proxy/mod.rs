//! Public 代理命令组件的稳定出口。
//! 入口只负责根据服务端投影选择创建、降低上限或成功反馈视图；各表单自己的状态和
//! 提交逻辑放在子模块中，命令 ID 与 API 调用仍由对应表单独占。

mod authorization;
mod create;
mod success;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::pending::ProxyConfirmCommand;
use super::session::CommandSession;
use crate::api::{OwnedProxy, ProxyCommandAuthorization};
use authorization::ProxyAuthorizationForm;
use create::ProxyCreateForm;
use success::proxy_success_panel;

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
    pub saved_authorization: Signal<Option<Box<ProxyCommandAuthorization>>>,
    /// 由弹窗宿主注入的两阶段确认命令 hook（创建/降低上限各一份，整体 Copy）；
    /// 宿主据其 `submitting` 接线 Dialog 的 busy 关闭守卫，避免弹窗 scope 销毁
    /// 导致成功后的刷新回调丢失。
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
        saved_authorization,
        create_command,
        lower_command,
        on_close,
    } = input;
    let session = use_context::<CommandSession>();
    let last_success = use_hook(|| Rc::new(RefCell::new(None::<Option<OwnedProxy>>)));
    if let Some(authorization) = saved_authorization() {
        return proxy_success_panel(&authorization, on_close);
    }
    // 不确定结果必须优先于新快照的 create/lower 分支，避免已生效操作失去幂等重试入口。
    let uncertain_command = if (create_command.uncertain)() {
        Some(create_command)
    } else if (lower_command.uncertain)() {
        Some(lower_command)
    } else {
        None
    };
    if let Some(command) = uncertain_command {
        return rsx! { super::retry::ProxyRetry { lot_id, command, saved_authorization, on_close } };
    }
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
                saved_authorization,
                command: lower_command,
                peer_submitting: create_command.submitting,
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
                saved_authorization,
                command: create_command,
                peer_submitting: lower_command.submitting,
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
