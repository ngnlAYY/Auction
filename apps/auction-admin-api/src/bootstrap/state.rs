//! Admin API 的状态装配入口。

use std::sync::Arc;

#[cfg(feature = "postgres")]
mod database;
mod fallback;
#[cfg(feature = "postgres")]
mod heartbeat;

use auction_config::AppConfig;
use auction_http::AdminApiState;
use auction_observability::ProcessMetrics;

/// 按当前制品 feature 选择数据库完整档或安全降级状态装配。
pub(super) async fn configure_state(
    config: &AppConfig,
    tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
) -> anyhow::Result<(
    AdminApiState,
    Option<Arc<auction_observability::HeartbeatPublisher>>,
    Option<auction_process::TaskGuard<()>>,
)> {
    #[cfg(feature = "postgres")]
    {
        database::configure_state(config, tracing_health, metrics).await
    }
    #[cfg(not(feature = "postgres"))]
    {
        let (state, heartbeat) = fallback::configure_state(config, tracing_health, metrics).await?;
        Ok((state, heartbeat, None))
    }
}

#[cfg(feature = "postgres")]
/// 启动 Admin API 的周期 heartbeat。
pub(super) fn spawn_heartbeat(
    publisher: Arc<auction_observability::HeartbeatPublisher>,
) -> tokio::task::JoinHandle<()> {
    heartbeat::spawn_heartbeat(publisher)
}
