//! Webhook 告警传输：只发送已冻结的 JSON；不读取 Cookie、不重试、不跟随重定向。

use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::operations::{
    OperationsAlertWebhookOutcome, OperationsAlertWebhookSender,
};
use reqwest::{Client, redirect::Policy};
use serde_json::Value;
use url::Url;

/// 受限制的 HTTPS webhook 客户端；地址与响应正文都不进入日志。
#[derive(Clone)]
pub struct OperationsAlertWebhook {
    target: Url,
}

impl std::fmt::Debug for OperationsAlertWebhook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OperationsAlertWebhook([REDACTED])")
    }
}

impl OperationsAlertWebhook {
    /// 只接受绝对 HTTPS 地址，拒绝用户信息、片段和控制字符。
    pub fn new(address: &str) -> Result<Self, OperationsAlertWebhookConfigError> {
        let target = target(address).ok_or(OperationsAlertWebhookConfigError)?;
        Ok(Self { target })
    }
}

/// webhook 地址无效；错误不携带地址或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertWebhookConfigError;

impl std::fmt::Display for OperationsAlertWebhookConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("operations alert webhook address is invalid")
    }
}

impl std::error::Error for OperationsAlertWebhookConfigError {}

impl OperationsAlertWebhookSender for OperationsAlertWebhook {
    fn send(
        &self,
        payload: Value,
        command_id: uuid::Uuid,
    ) -> ApplicationFuture<'_, OperationsAlertWebhookOutcome> {
        Box::pin(async move {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let client = Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .build();
            let Ok(client) = client else {
                return Ok(OperationsAlertWebhookOutcome::Unknown);
            };
            let Ok(body) = serde_json::to_vec(&payload) else {
                return Ok(OperationsAlertWebhookOutcome::Unknown);
            };
            match client
                .post(self.target.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", command_id.to_string())
                .body(body)
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    Ok(OperationsAlertWebhookOutcome::Succeeded)
                }
                Ok(response)
                    if response.status().is_server_error()
                        || response.status().as_u16() == 408
                        || response.status().as_u16() == 429 =>
                {
                    Ok(OperationsAlertWebhookOutcome::Retryable)
                }
                Ok(_) => Ok(OperationsAlertWebhookOutcome::Permanent),
                // 传输失败无法证明请求没有到达远端；只能人工核对，不能自动重发。
                Err(error) => {
                    drop(error);
                    Ok(OperationsAlertWebhookOutcome::Unknown)
                }
            }
        })
    }
}

fn target(address: &str) -> Option<Url> {
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

#[cfg(test)]
mod tests {
    use super::target;

    #[test]
    fn rejects_non_https_and_userinfo() {
        assert!(target("http://example.invalid/hook").is_none());
        assert!(target("https://user:pass@example.invalid/hook").is_none());
        assert!(target("https://example.invalid/hook#fragment").is_none());
        assert!(target("https://example.invalid/hook").is_some());
    }
}
