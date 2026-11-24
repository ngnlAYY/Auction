//! PostgreSQL 持久化 Worker 的装配与退出收口。

use std::sync::Arc;

use anyhow::Context;
use auction_application::operations::ExternalWriteLifecycle;
use auction_application::runtime_switches::RuntimeSwitches;
use auction_config::AppConfig;
use auction_observability::{ProcessMetrics, TracingHealthReader};
use auction_storage::DatabaseStore;
use uuid::Uuid;

use crate::bootstrap::ShutdownFuture;
use crate::bootstrap::skeleton::run_worker_skeleton;

mod consignment;
mod heartbeat;
mod profile;
mod recovery;
mod run;
#[cfg(test)]
pub(super) use consignment::consignment_test_config;

struct PersistentRuntime {
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    switches: RuntimeSwitches,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    write_lifecycle: ExternalWriteLifecycle,
    shutdown: ShutdownFuture,
}

/// 装配 PostgreSQL Worker，连接后先完成寄售启动预检。
pub(super) async fn run_worker(
    config: &AppConfig,
    tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    switches: RuntimeSwitches,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    write_lifecycle: ExternalWriteLifecycle,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    let Some(database_url) = config.database_url() else {
        if config.worker.mooglemail.enabled
            || config.worker.forum.enabled
            || config.worker.operations_alert.notify_enabled()
            || config.worker.operations_alert.webhook_enabled()
        {
            anyhow::bail!("启用外部 Worker 功能时必须配置 PostgreSQL database_url");
        }
        run_worker_skeleton(config, metrics, runtime_failure, shutdown).await?;
        return Ok(());
    };
    // 审计失败按类别向共享影子框架记样本；进程级安装一次。
    auction_storage::install_audit_shadow_recorder(metrics.shadow());
    let store = Arc::new(
        DatabaseStore::connect(database_url, profile::database_profile(config)?)
            .await
            .context("连接 Worker 数据库失败")?
            .with_database_recorder(metrics.database())
            .with_runtime_switches(switches.clone())
            .with_notification_recorder(metrics.notifications())
            .with_shadow_recorder(metrics.shadow())
            .with_public_base_url(config.worker.forum.public_base_url.clone())
            .with_migration_compatibility_mode(config.readiness.migration_compatibility_mode)
            .with_migration_readiness_recorder(metrics.migration_readiness())
            .with_delivery_send_intervals(config.worker.mooglemail.send_intervals)
            .context("配置投递发送间隔失败")?,
    );
    let worker_id = format!("auction-worker-{}", Uuid::new_v4());
    let runtime = PersistentRuntime {
        tracing_health,
        metrics,
        switches: switches.clone(),
        runtime_failure,
        write_lifecycle,
        shutdown,
    };
    consignment::start_persistent(
        config,
        consignment::ConsignmentPorts::postgres(&store),
        &switches,
        &worker_id,
        store.has_consignment_work(),
        consignment::hvdata_lookup,
        async |consignment| {
            run::run_connected(config, store.clone(), worker_id.clone(), runtime, consignment).await
        },
    )
    .await
}
