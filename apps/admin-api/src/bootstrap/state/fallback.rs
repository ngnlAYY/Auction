//! Admin API 未启用数据库 feature 时的安全降级装配。

use std::sync::Arc;

use auction_config::AppConfig;
use auction_http::AdminApiState;
use auction_integrations::ForumPreviewAdapter;
use auction_observability::ProcessMetrics;

use super::super::config;

/// 仅提供启动骨架能力，并拒绝连接数据库的配置。
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
    Ok((
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
        .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config)),
        None,
    ))
}
