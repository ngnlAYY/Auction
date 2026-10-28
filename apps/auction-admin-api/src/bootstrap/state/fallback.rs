//! Admin API 无数据库时的共用安全降级装配。

use std::sync::Arc;

use auction_application::WorkerControlPort;
use auction_config::AppConfig;
use auction_http::{AdminApiState, AdminHttpSecurity};
use auction_integrations::ForumPreviewAdapter;
#[cfg(not(feature = "postgres"))]
use auction_observability::ProcessMetrics;

use super::super::config;

/// 仅提供启动骨架能力，并拒绝连接数据库的配置。
#[cfg(not(feature = "postgres"))]
pub(super) async fn configure_state(
    config: &AppConfig,
    _tracing_health: Arc<dyn auction_observability::TracingHealthReader>,
    _metrics: Arc<ProcessMetrics>,
) -> anyhow::Result<(AdminApiState, Option<Arc<auction_observability::HeartbeatPublisher>>)> {
    let security = config::configure_security(config)?;
    let worker_control = config::configure_worker_control(config);
    if config.database_url().is_some() {
        anyhow::bail!("admin-api 配置了数据库，但当前制品未编译 postgres feature");
    }
    Ok((empty_state(config, security, worker_control), None))
}

/// 两种 feature 路径共用同一端口集合；安全校验与缺失 feature 错误仍由调用方按原顺序处理。
pub(super) fn empty_state(
    config: &AppConfig,
    security: AdminHttpSecurity,
    worker_control: Option<Arc<dyn WorkerControlPort>>,
) -> AdminApiState {
    AdminApiState::new(
        None,
        None,
        None,
        None,
        None,
        None,
        Some(Arc::new(ForumPreviewAdapter::new())),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        worker_control,
        security,
    )
    .with_dashboard_worlds(config::configured_dashboard_worlds(config))
    .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config))
}
