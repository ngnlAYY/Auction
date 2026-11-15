//! Webhook 告警传输：只发送已冻结的 JSON；不读取 Cookie、不重试、不跟随重定向。

use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::DiagnosticContext;
use auction_application::operations::{
    OperationsAlertWebhookOutcome, OperationsAlertWebhookSender,
};
use reqwest::Client;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::outbound::{https_target, restricted_client_builder};
use crate::safe_diagnostics;

/// 受限制的 HTTPS webhook 客户端；地址与响应正文都不进入日志。
/// 客户端在构造时建立一次，后续发送复用同一 TLS 根证书库，但每次发送都新建连接。
#[derive(Clone)]
pub struct OperationsAlertWebhook {
    target: Url,
    client: Client,
}

impl std::fmt::Debug for OperationsAlertWebhook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OperationsAlertWebhook([REDACTED])")
    }
}

impl OperationsAlertWebhook {
    /// 只接受绝对 HTTPS 地址，拒绝用户信息、片段和控制字符；受限客户端无法构造时同样
    /// 作为配置错误在启动期失败，而不是每次发送都退化为结果未知。
    pub fn new(address: &str) -> Result<Self, OperationsAlertWebhookConfigError> {
        let target =
            https_target(address).ok_or(OperationsAlertWebhookConfigError::InvalidAddress)?;
        let client = build_client().map_err(safe_diagnostics::error_map(
            "operations_alert_webhook",
            "build_client",
            || OperationsAlertWebhookConfigError::ClientUnavailable,
        ))?;
        Ok(Self { target, client })
    }
}

/// 不保留空闲连接：复用连接上的写出后失败不属于连接错误，只能判结果未知并转人工核对；
/// 每次新建连接时，连接建立前的失败一律可证明未发送、按可重试处理。告警稀疏，握手成本可忽略。
fn build_client() -> Result<Client, reqwest::Error> {
    restricted_client_builder().pool_max_idle_per_host(0).build()
}

/// webhook 配置无效；错误不携带地址或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OperationsAlertWebhookConfigError {
    /// 地址不是不带用户信息与片段的绝对 HTTPS URL。
    #[error("operations alert webhook address is invalid")]
    InvalidAddress,
    /// 受限 HTTPS 客户端无法构造（例如系统 TLS 根证书不可用）。
    #[error("operations alert webhook HTTPS client could not be built")]
    ClientUnavailable,
}

impl OperationsAlertWebhookSender for OperationsAlertWebhook {
    fn send(
        &self,
        payload: Value,
        command_id: uuid::Uuid,
    ) -> ApplicationFuture<'_, OperationsAlertWebhookOutcome> {
        Box::pin(async move {
            let Ok(body) = serde_json::to_vec(&payload) else {
                return Ok(OperationsAlertWebhookOutcome::Unknown);
            };
            match self
                .client
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
                // 连接建立阶段（DNS、TCP、TLS 握手）失败时请求字节尚未写出，属于已知未发送：
                // 按退避重试，次数仍受尝试上限约束，且请求带同一 Idempotency-Key。
                // 分类只依据 is_connect；客户端不复用空闲连接，不会把请求写进已被对端关闭的旧连接。
                Err(error) if error.is_connect() => {
                    safe_diagnostics::warn(
                        "operations_alert_webhook",
                        "connect",
                        safe_diagnostics::project(&error),
                        DiagnosticContext::default(),
                    );
                    Ok(OperationsAlertWebhookOutcome::Retryable)
                }
                // 请求写出后的传输失败无法证明远端没有收到；只能人工核对，不能自动重发。
                Err(error) => {
                    safe_diagnostics::warn(
                        "operations_alert_webhook",
                        "send_outcome_unknown",
                        safe_diagnostics::project(&error),
                        DiagnosticContext::default(),
                    );
                    Ok(OperationsAlertWebhookOutcome::Unknown)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use auction_application::operations::{
        OperationsAlertWebhookOutcome, OperationsAlertWebhookSender,
    };
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::{OperationsAlertWebhook, OperationsAlertWebhookConfigError};

    #[test]
    fn invalid_address_fails_at_construction() {
        for address in [
            "http://example.invalid/hook",
            "https://user:pass@example.invalid/hook",
            "https://example.invalid/hook#fragment",
        ] {
            assert_eq!(
                OperationsAlertWebhook::new(address).err(),
                Some(OperationsAlertWebhookConfigError::InvalidAddress),
                "{address}"
            );
        }
        assert!(OperationsAlertWebhook::new("https://example.invalid/hook").is_ok());
    }

    /// 端口已释放：连接被拒绝时请求从未写出，按可重试处理而不是转人工核对。
    #[tokio::test]
    async fn refused_connection_is_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        drop(listener);
        let webhook = OperationsAlertWebhook::new(&format!("https://127.0.0.1:{port}/hook"))
            .expect("webhook");

        let outcome = webhook
            .send(serde_json::json!({ "alert": "fixture" }), uuid::Uuid::new_v4())
            .await
            .expect("outcome");
        assert_eq!(outcome, OperationsAlertWebhookOutcome::Retryable);
    }

    /// TLS 握手失败同属连接建立阶段：HTTP 请求尚未写出，仍按可重试处理。
    #[tokio::test]
    async fn failed_tls_handshake_is_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            stream.write_all(b"not a tls server\r\n").await.expect("write");
            stream.shutdown().await.expect("shutdown");
        });
        let webhook = OperationsAlertWebhook::new(&format!("https://127.0.0.1:{port}/hook"))
            .expect("webhook");

        let outcome = webhook
            .send(serde_json::json!({ "alert": "fixture" }), uuid::Uuid::new_v4())
            .await
            .expect("outcome");
        assert_eq!(outcome, OperationsAlertWebhookOutcome::Retryable);
        server.await.expect("server task");
    }
}
