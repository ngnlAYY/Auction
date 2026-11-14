//! 备份打卡：仅一次 HTTPS GET，超时和传输错误全部收敛为脱敏结果。

use std::time::Duration;

use auction_application::core::diagnostics::DiagnosticContext;
use reqwest::{Client, redirect::Policy};
use url::Url;

use crate::safe_diagnostics;

/// 打卡结果闭集；失败不改变备份自身的成败。
#[derive(Debug, Clone, Copy)]
pub enum BackupHeartbeatStatus {
    /// 服务返回成功状态码。
    Sent,
    /// 输入、传输、超时或 HTTP 状态失败；不携带底层错误和地址。
    Failed,
    /// 地址省略或为空，未创建客户端，也未发起请求。
    NotConfigured,
}

/// 只供调用方输出日志的打卡结果；永远不包含 URL、认证值或响应正文。
#[derive(Debug, Clone, Copy)]
pub struct BackupHeartbeatResult {
    /// 是否已发送、失败或未配置。
    pub status: BackupHeartbeatStatus,
    /// 收到响应时的状态码；连接或客户端构造失败时为空。
    pub http_status: Option<u16>,
}

/// 只消费显式配置的地址；备份打卡与 Worker 存活检查共用同一受限 HTTPS GET。
/// 禁止代理环境、重定向、自动重试和 Cookie。
pub async fn send_heartbeat(address: Option<&str>) -> BackupHeartbeatResult {
    let Some(address) = address.filter(|address| !address.is_empty()) else {
        return BackupHeartbeatResult {
            status: BackupHeartbeatStatus::NotConfigured,
            http_status: None,
        };
    };
    let Some(target) = target(address) else {
        return failed(None);
    };
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
        return failed(None);
    };
    match client.get(target).send().await {
        Ok(response) if response.status().is_success() => BackupHeartbeatResult {
            status: BackupHeartbeatStatus::Sent,
            http_status: Some(response.status().as_u16()),
        },
        Ok(response) => failed(Some(response.status().as_u16())),
        Err(diagnostic_cause) => {
            safe_diagnostics::warn(
                "backup_heartbeat",
                "send_heartbeat",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            failed(None)
        }
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

fn failed(http_status: Option<u16>) -> BackupHeartbeatResult {
    BackupHeartbeatResult { status: BackupHeartbeatStatus::Failed, http_status }
}

#[cfg(test)]
mod tests {
    use super::{BackupHeartbeatStatus, send_heartbeat};

    #[tokio::test]
    async fn unconfigured_worker_heartbeat_is_a_noop() {
        let result = send_heartbeat(None).await;
        assert!(matches!(result.status, BackupHeartbeatStatus::NotConfigured));
        assert!(result.http_status.is_none());
    }

    #[tokio::test]
    async fn invalid_worker_heartbeat_address_fails_without_network() {
        let result = send_heartbeat(Some("http://example.invalid/heartbeat")).await;
        assert!(matches!(result.status, BackupHeartbeatStatus::Failed));
        assert!(result.http_status.is_none());
    }
}
