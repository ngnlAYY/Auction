//! Public 代理命令组件的稳定出口。
//!
//! 入口只负责根据服务端投影选择创建、降低上限或成功反馈视图；各表单自己的状态和
//! 提交逻辑放在子模块中，命令 ID 与 API 调用仍由对应表单独占。

mod authorization;
mod create;
mod success;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::{OwnedProxy, ProxyCommandAuthorization};

use super::super::RetryPlaceholder;
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
