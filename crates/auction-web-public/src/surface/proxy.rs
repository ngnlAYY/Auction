//! Public 代理授权的展示文案。
//! 本模块只格式化服务端已经过滤的代理投影，不在浏览器端推导授权状态机或竞价结果。

use auction_web_shared::t;

use super::format_credit_amount;
use crate::api::OwnedProxy;

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
                format_credit_amount(transition.new_maximum_amount),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
