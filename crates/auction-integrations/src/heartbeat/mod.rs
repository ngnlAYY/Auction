//! 备份打卡：仅一次 HTTPS GET，超时和传输错误全部收敛为脱敏结果。地址与受限客户端在构造时
//! 冻结，Worker 存活检查循环复用同一连接池与 TLS 根证书库，不必每次重新加载系统证书。

use auction_application::core::diagnostics::DiagnosticContext;
use reqwest::Client;
use url::Url;

use crate::outbound::{https_target, restricted_client_builder};
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

/// 构造时确定的打卡目标；地址无效或客户端无法构造时每次发送都直接失败，不发网络。
enum HeartbeatTarget {
    NotConfigured,
    Unavailable,
    Ready { target: Url, client: Client },
}

/// 备份打卡与 Worker 存活检查共用的受限 HTTPS GET；禁止代理环境、重定向、自动重试和 Cookie。
/// 长期持有时复用同一客户端，只按状态码判定结果。
pub struct BackupHeartbeat {
    target: HeartbeatTarget,
}

impl BackupHeartbeat {
    /// 只消费显式配置的地址；省略或为空即未配置，不创建客户端。
    pub fn new(address: Option<&str>) -> Self {
        let Some(address) = address.filter(|address| !address.is_empty()) else {
            return Self { target: HeartbeatTarget::NotConfigured };
        };
        let Some(target) = https_target(address) else {
            return Self { target: HeartbeatTarget::Unavailable };
        };
        match restricted_client_builder().build() {
            Ok(client) => Self { target: HeartbeatTarget::Ready { target, client } },
            Err(diagnostic_cause) => {
                // 客户端只在构造时创建一次：失败原因只在这里有，之后每次打卡只报失败。
                safe_diagnostics::error(
                    "backup_heartbeat",
                    "build_client",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                Self { target: HeartbeatTarget::Unavailable }
            }
        }
    }

    /// 发送一次打卡。
    pub async fn send(&self) -> BackupHeartbeatResult {
        let (target, client) = match &self.target {
            HeartbeatTarget::NotConfigured => {
                return BackupHeartbeatResult {
                    status: BackupHeartbeatStatus::NotConfigured,
                    http_status: None,
                };
            }
            HeartbeatTarget::Unavailable => return failed(None),
            HeartbeatTarget::Ready { target, client } => (target, client),
        };
        match client.get(target.clone()).send().await {
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
}

impl std::fmt::Debug for BackupHeartbeat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BackupHeartbeat([REDACTED])")
    }
}

/// 一次性打卡：构造受限客户端、发送一次后丢弃；供只打一次卡的调用方（CLI 定时备份）使用。
pub async fn send_heartbeat(address: Option<&str>) -> BackupHeartbeatResult {
    BackupHeartbeat::new(address).send().await
}

fn failed(http_status: Option<u16>) -> BackupHeartbeatResult {
    BackupHeartbeatResult { status: BackupHeartbeatStatus::Failed, http_status }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use tokio::net::TcpListener;

    use super::{BackupHeartbeat, BackupHeartbeatStatus, send_heartbeat};

    #[tokio::test]
    async fn unconfigured_worker_heartbeat_is_a_noop() {
        let result = send_heartbeat(None).await;
        assert_matches!(result.status, BackupHeartbeatStatus::NotConfigured);
        assert!(result.http_status.is_none());
        let empty = BackupHeartbeat::new(Some("")).send().await;
        assert_matches!(empty.status, BackupHeartbeatStatus::NotConfigured);
    }

    #[tokio::test]
    async fn invalid_worker_heartbeat_address_fails_without_network() {
        let result = send_heartbeat(Some("http://example.invalid/heartbeat")).await;
        assert_matches!(result.status, BackupHeartbeatStatus::Failed);
        assert!(result.http_status.is_none());
    }

    /// 长期持有的打卡器每次发送都复用构造时冻结的目标；连接失败只报告失败，不携带状态码。
    #[tokio::test]
    async fn reused_heartbeat_reports_each_failed_send() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        drop(listener);
        let heartbeat = BackupHeartbeat::new(Some(&format!("https://127.0.0.1:{port}/heartbeat")));
        assert_eq!(format!("{heartbeat:?}"), "BackupHeartbeat([REDACTED])");
        for _ in 0..2 {
            let result = heartbeat.send().await;
            assert_matches!(result.status, BackupHeartbeatStatus::Failed);
            assert!(result.http_status.is_none());
        }
    }
}
