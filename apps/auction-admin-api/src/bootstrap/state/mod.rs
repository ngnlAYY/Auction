//! Admin API 的状态装配入口。

use std::sync::Arc;

mod database;
mod fallback;
mod heartbeat;

use auction_config::AppConfig;
use auction_http::AdminApiState;
use auction_observability::ProcessMetrics;

/// 装配 Admin 状态；未配置数据库时由 `database` 回落到只含安全降级端口的空状态。
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> anyhow::Result<(
    AdminApiState,
    Option<Arc<auction_observability::HeartbeatPublisher>>,
    Option<auction_process::TaskGuard<()>>,
)> {
    database::configure_state(config, tracing_health, metrics, switches).await
}

/// 启动 Admin API 的周期 heartbeat。
pub(super) fn spawn_heartbeat(
    publisher: Arc<auction_observability::HeartbeatPublisher>,
) -> tokio::task::JoinHandle<()> {
    heartbeat::spawn_heartbeat(publisher)
}
