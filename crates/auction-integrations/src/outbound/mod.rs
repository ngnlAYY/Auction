//! 受限出站 HTTPS 的唯一 owner：外部打卡与告警 Webhook 共用的地址校验与客户端约束。
//! 只允许绝对 HTTPS 地址；客户端不走代理环境、不跟随重定向、不自动重试，连接 3 秒、
//! 整体 5 秒超时。调用方只能在此基础上收紧（如关闭空闲连接复用），不能放宽。

use std::time::Duration;

use reqwest::{Client, ClientBuilder, redirect::Policy};
use url::Url;

/// 只接受不带用户信息与片段、无首尾空白与控制字符的绝对 HTTPS 地址；不合格返回 `None`。
/// 地址可能含访问令牌，调用方不得把原文写进日志或错误。
pub(crate) fn https_target(address: &str) -> Option<Url> {
    if address.len() > 4096 || address.trim() != address || address.chars().any(char::is_control) {
        return None;
    }
    let target = Url::parse(address).ok()?;
    let authority = address.split_once("://")?.1.split(['/', '?', '#']).next()?;
    (target.scheme() == "https"
        && target.host_str().is_some()
        && target.username().is_empty()
        && target.password().is_none()
        && !authority.contains('@')
        && target.fragment().is_none())
    .then_some(target)
}

/// 受限客户端的构造器；`build()` 读取系统 TLS 根证书，调用方应构造一次后复用客户端。
pub(crate) fn restricted_client_builder() -> ClientBuilder {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // 装备来源依赖 reqwest 默认的证书链和主机名校验，此处两项均必须启用。
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
}

#[cfg(test)]
mod tests {
    use super::{https_target, restricted_client_builder};

    #[test]
    fn https_target_rejects_non_https_userinfo_fragment_and_padding() {
        assert!(https_target("http://example.invalid/hook").is_none());
        assert!(https_target("https://user:pass@example.invalid/hook").is_none());
        assert!(https_target("https://user@example.invalid/hook").is_none());
        assert!(https_target("https://example.invalid/hook#fragment").is_none());
        assert!(https_target(" https://example.invalid/hook").is_none());
        assert!(https_target("https://example.invalid/ho\nok").is_none());
        assert!(https_target("not a url").is_none());
        assert!(https_target("https://example.invalid/hook?token=fixture").is_some());
    }

    #[test]
    fn https_target_rejects_oversized_address() {
        let address = format!("https://example.invalid/{}", "a".repeat(4096));
        assert!(https_target(&address).is_none());
    }

    #[test]
    fn restricted_client_can_be_built() {
        assert!(restricted_client_builder().build().is_ok());
    }
}
