//! Admin API 无数据库时的共用安全降级装配。

use std::sync::Arc;

use auction_application::operations::WorkerControlPort;
use auction_config::AppConfig;
use auction_http::{AdminApiState, AdminHttpSecurity};

use super::super::config;

/// 未配置数据库时的空端口集合；安全校验仍由调用方先完成。
pub(super) fn empty_state(
    config: &AppConfig,
    security: AdminHttpSecurity,
    worker_control: Option<Arc<dyn WorkerControlPort>>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
) -> AdminApiState {
    AdminApiState::new(
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
    .with_operations_alert_policy(config.worker.operations_alert.policy())
    .with_dashboard_mailbox_keys(config::configured_dashboard_mailbox_keys(config))
    .with_runtime_switch_snapshot(switches)
}
