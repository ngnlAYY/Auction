//! Public 代理命令组件的稳定出口。
//! 入口只负责根据服务端投影选择创建、降低上限或成功反馈视图；各表单自己的状态和
//! 提交逻辑放在子模块中，命令 ID 与 API 调用仍由对应表单独占。

mod authorization;
mod create;
mod success;

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{OwnedProxy, ProxyCommandAuthorization};

use super::super::{AsyncState, AsyncStateKind};
use super::common::ProxyConfirmCommand;
use authorization::ProxyAuthorizationForm;
use create::ProxyCreateForm;
use success::proxy_success_panel;

/// 公开 lot 的代理授权表单参数；账户状态由服务端投影，不在浏览器端复制代理状态机。
pub struct ProxyFormInput {
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
    /// 由弹窗宿主注入的两阶段确认命令 hook（创建/降低上限各一份，整体 Copy）；
    /// 宿主据其 `submitting` 接线 Dialog 的 busy 关闭守卫，避免弹窗 scope 销毁
    /// 导致成功后的刷新回调丢失。
    pub create_command: ProxyConfirmCommand,
    pub lower_command: ProxyConfirmCommand,
    pub on_close: EventHandler<()>,
}

/// 公开 lot 的代理授权入口。
pub fn proxy_form(input: ProxyFormInput) -> Element {
    let ProxyFormInput {
        lot_id,
        current_price,
        next_bid,
        lot_status,
        allow_slow,
        leading,
        blocked_reason,
        proxy,
        mut refresh_generation,
        saved_authorization,
        create_command,
        lower_command,
        on_close,
    } = input;
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
    match proxy.as_ref() {
        None => rsx! { p { role: "status", {t!("proxy-loading")} } },
        Some(Err(_)) => rsx! {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("catalog-proxy"),
                detail: t!("proxy-load-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    refresh_generation.set(refresh_generation().saturating_add(1));
                },
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
                command: create_command,
                peer_submitting: lower_command.submitting,
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
                command: lower_command,
                peer_submitting: create_command.submitting,
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
                command: create_command,
                peer_submitting: lower_command.submitting,
                on_close,
            }
        },
    }
}
