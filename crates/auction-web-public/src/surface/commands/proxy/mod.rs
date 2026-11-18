//! Public 代理命令组件的稳定出口。
//! 根据权威授权快照选择创建或降低上限的确认表单。

mod authorization;
mod create;
#[cfg(test)]
mod tests;

use auction_web_shared::{AsyncState, AsyncStateKind, t};
use dioxus::prelude::*;

use super::pending::{PendingProxyCommand, ProxyConfirmCommand};
use super::session::CommandSession;
use crate::api::OwnedProxy;
use authorization::ProxyAuthorizationForm;
use create::ProxyCreateForm;

/// 本轮读取的结果与启动它的刷新代号，独立于最后确认的授权快照。
pub(in crate::surface) type ProxyAuthorizationRead =
    Resource<Option<(u64, Result<Option<OwnedProxy>, String>)>>;

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
    pub proxy: Option<Option<OwnedProxy>>,
    pub proxy_read: Option<ProxyAuthorizationRead>,
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
        proxy_read,
        mut refresh_generation,
        on_submit,
        create_command,
        lower_command,
        on_close,
    } = input;
    let session = use_context::<CommandSession>();
    let displayed = proxy.clone();
    let authorize = use_callback(move |()| {
        // 二次确认也复核读取状态，失效或请求重启先于重绘时不能沿用旧授权写入。
        can_bid
            && proxy_read_status(proxy_read, refresh_generation(), displayed.as_ref())
                == ProxyReadStatus::Confirmed
    });
    let read_status = proxy_read_status(proxy_read, refresh_generation(), proxy.as_ref());
    if !session.identity_confirmed() {
        return rsx! { p { role: "status", {t!("bid-unavailable")} } };
    }
    let authorization_ready = can_bid && read_status == ProxyReadStatus::Confirmed;
    let form = match proxy.as_ref() {
        Some(Some(proxy)) if proxy.status == "active" => rsx! {
            ProxyAuthorizationForm {
                lot_id,
                current_price,
                next_bid,
                lot_status,
                proxy: proxy.clone(),
                authorization_ready,
                authorize,
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
                authorize,
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
        if read_status == ProxyReadStatus::Failed {
            AsyncState {
                kind: AsyncStateKind::Error,
                title: t!("catalog-proxy"),
                detail: t!("proxy-load-error"),
                retry_label: t!("data-retry"),
                on_retry: move |_| {
                    // 推进代号立即撤销本轮授权，旧投影仍可用于展示。
                    let next = refresh_generation.peek().saturating_add(1);
                    refresh_generation.set(next);
                },
            }
        } else if can_bid && read_status == ProxyReadStatus::Loading {
            p { role: "status", {t!("proxy-loading")} }
        }
    }
}

#[derive(PartialEq, Eq)]
enum ProxyReadStatus {
    Loading,
    Confirmed,
    Failed,
}

fn proxy_read_status(
    read: Option<ProxyAuthorizationRead>,
    generation: u64,
    displayed: Option<&Option<OwnedProxy>>,
) -> ProxyReadStatus {
    let Some(read) = read else { return ProxyReadStatus::Loading };
    if !read.state().try_read().is_ok_and(|state| *state == UseResourceState::Ready) {
        return ProxyReadStatus::Loading;
    }
    let Ok(read) = read.try_read() else { return ProxyReadStatus::Loading };
    match read.as_ref().and_then(Option::as_ref) {
        Some((checked, Ok(snapshot))) if *checked == generation && Some(snapshot) == displayed => {
            ProxyReadStatus::Confirmed
        }
        Some((checked, Err(_error))) if *checked == generation => ProxyReadStatus::Failed,
        _ => ProxyReadStatus::Loading,
    }
}
