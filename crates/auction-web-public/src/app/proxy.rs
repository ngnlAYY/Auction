//! Public 代理授权的安全展示组件。
//!
//! 本模块只格式化服务端已经过滤的代理投影，不在浏览器端推导授权状态机或竞价结果。

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::OwnedProxy;

use super::format_amount;

/// 在 lot 详情中展示当前用户的代理授权；加载失败保留显式重试入口。
#[component]
pub(super) fn ProxySummary(
    snapshot: Option<Result<Option<OwnedProxy>, String>>,
    mut refresh_generation: Signal<u64>,
) -> Element {
    let title = t!("proxy-summary-title");
    rsx! {
        section {
            class: "public-auction-proxy-summary",
            aria_label: title.clone(),
            h2 { "{title}" }
            match snapshot {
                None => rsx! {
                    p { role: "status", {t!("proxy-loading")} }
                },
                Some(Err(_)) => rsx! {
                    div { class: "public-auction-proxy-summary__error", role: "alert",
                        p { {t!("proxy-load-error")} }
                        button {
                            r#type: "button",
                            onclick: move |_| {
                                refresh_generation
                                    .set(refresh_generation().saturating_add(1));
                            },
                            {t!("data-retry")}
                        }
                    }
                },
                Some(Ok(None)) => rsx! {
                    p { class: "public-auction-lot__none", {t!("proxy-summary-not-set")} }
                },
                Some(Ok(Some(proxy))) => proxy_details(proxy),
            }
        }
    }
}

fn proxy_details(proxy: OwnedProxy) -> Element {
    let quota = proxy_quota_label(&proxy);
    let capability = proxy_capability_label(&proxy);
    let transitions = proxy_transition_summary(&proxy);
    let status = proxy_status_label(&proxy.status);
    let status_class = match proxy.status.as_str() {
        "active" => "active",
        "final_fast" => "final-fast",
        "exhausted" => "exhausted",
        "frozen" => "frozen",
        "closed" => "closed",
        _ => "unknown",
    };

    rsx! {
        dl {
            dt { {t!("account-maximum")} }
            dd { class: "public-auction-lot__price", "{format_amount(proxy.maximum_amount)}" }
            dt { {t!("account-mode")} }
            dd { {proxy_mode_label(&proxy.mode)} }
            dt { {t!("account-quota")} }
            dd { "{quota}" }
            dt { {t!("account-capability")} }
            dd { "{capability}" }
            dt { {t!("account-transition-history")} }
            dd { class: "public-auction-proxy-summary__history", "{transitions}" }
            dt { {t!("account-status")} }
            dd {
                span {
                    class: "public-auction-proxy-status public-auction-proxy-status--{status_class}",
                    "{status}"
                }
            }
        }
    }
}

pub(super) fn proxy_mode_label(mode: &str) -> String {
    match mode {
        "fast" => t!("proxy-mode-fast"),
        "slow" => t!("proxy-mode-slow"),
        _ => t!("value-empty"),
    }
}

pub(super) fn proxy_transition_label(kind: &str) -> String {
    match kind {
        "created" => t!("account-transition-created"),
        "lowered" => t!("account-transition-lowered"),
        "final_fast" => t!("account-transition-final-fast"),
        "exhausted" => t!("account-transition-exhausted"),
        "frozen" => t!("account-transition-frozen"),
        "closed" => t!("account-transition-closed"),
        _ => t!("value-empty"),
    }
}

pub(super) fn proxy_status_label(status: &str) -> String {
    match status {
        "active" => t!("account-proxy-status-active"),
        "final_fast" => t!("account-proxy-status-final-fast"),
        "exhausted" => t!("account-proxy-status-exhausted"),
        "frozen" => t!("account-proxy-status-frozen"),
        "closed" => t!("account-proxy-status-closed"),
        _ => t!("value-empty"),
    }
}

pub(super) fn proxy_quota_label(proxy: &OwnedProxy) -> String {
    if proxy.quota.limit == 0 {
        t!("account-quota-unlimited", used: proxy.quota.used)
    } else {
        t!(
            "account-quota-bounded",
            used: proxy.quota.used,
            limit: proxy.quota.limit,
            remaining: proxy.quota.limit.saturating_sub(proxy.quota.used)
        )
    }
}

pub(super) fn proxy_capability_label(proxy: &OwnedProxy) -> String {
    if proxy.capabilities.waiting {
        t!("account-capability-waiting")
    } else if proxy.capabilities.at_maximum {
        t!("account-capability-at-maximum")
    } else if proxy.capabilities.can_lower {
        t!("account-capability-lower")
    } else {
        t!("account-capability-locked")
    }
}

pub(super) fn proxy_transition_summary(proxy: &OwnedProxy) -> String {
    if proxy.transitions.is_empty() {
        return t!("value-empty");
    }
    proxy
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
}
