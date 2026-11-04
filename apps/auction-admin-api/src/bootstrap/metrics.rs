//! 附加指标任务只降级采集，不参与业务监督或 readiness。

use std::sync::Arc;

use auction_config::AppConfig;
use auction_observability::ProcessMetrics;
use auction_process::TaskGuard;
use tokio::net::TcpListener;

pub(super) fn start(
    config: &AppConfig,
    metrics: Arc<ProcessMetrics>,
    switches: auction_application::RuntimeSwitches,
) -> Option<TaskGuard<()>> {
    let address = config.metrics.loopback_listen_address(config.role)?;
    let role = config.role.name();
    Some(TaskGuard::new(tokio::spawn(async move {
        let listener = match TcpListener::bind(address).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(
                    kind = "metrics_bind_failed", role, io_kind = ?error.kind(),
                    "附加指标绑定失败；仅降级采集"
                );
                return;
            }
        };
        tracing::info!(role, %address, "附加指标出口已监听");
        let result =
            auction_http::serve_metrics(listener, metrics, switches, std::future::pending()).await;
        match result {
            Ok(()) => tracing::error!(kind = "metrics_serve_stopped", role, "附加指标服务意外停止"),
            Err(error) => tracing::error!(
                kind = "metrics_serve_failed", role, io_kind = ?error.kind(),
                "附加指标服务失败；仅降级采集"
            ),
        }
    })))
}
