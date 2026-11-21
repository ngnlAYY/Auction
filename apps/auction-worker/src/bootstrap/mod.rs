//! Worker 进程装配：加载角色配置、注入适配器、启动各 owner 运行时并处理优雅关闭。

use std::{ops::AsyncFnOnce, pin::Pin, sync::Arc};

use anyhow::Context;
use auction_application::core::Clock;
use auction_application::delivery::{DeliveryTransport, DeliveryWorkStore};
use auction_application::settlement::{
    SettlementCommandPort, SettlementFulfillmentCommandPort, SettlementFulfillmentEvidenceReader,
    SettlementFulfillmentStore, SettlementWorkStore,
};
use auction_config::{
    AppConfig, CONSIGNMENT_COMPLETE, ConsignmentActivation, Role, consignment_activation,
};
use auction_observability::{ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{TaskGuard, TerminationSignals};
use auction_time::Timestamp;

mod control;
mod forum;
mod fulfillment;
mod hot;
mod mail;
mod operations_alert;
mod persistent;
mod shutdown;
mod skeleton;

use persistent::run_worker;

type ShutdownFuture = Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

#[derive(Clone, Copy)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

type SettlementPorts = (
    Option<Arc<dyn SettlementWorkStore>>,
    Option<Arc<dyn SettlementCommandPort>>,
    Option<Arc<dyn SettlementFulfillmentStore>>,
    Option<Arc<dyn SettlementFulfillmentCommandPort>>,
    Option<Arc<dyn DeliveryWorkStore>>,
    Option<Arc<dyn DeliveryTransport>>,
    Option<Arc<dyn SettlementFulfillmentEvidenceReader>>,
);

/// 启动独立 Worker，沿用角色配置选择器。
pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("加载 Worker 配置失败")?;
    run_config(config, run_configured_service).await
}

/// 从显式角色路径启动，供单文件 supervisor 派生的独立进程使用。
pub async fn run_from_path(path: std::path::PathBuf) -> anyhow::Result<()> {
    let config =
        AppConfig::load_from_path(Role::Worker, path).context("加载显式指定的 Worker 配置失败")?;
    run_config(config, run_configured_service).await
}

async fn run_config(
    config: AppConfig,
    start: impl AsyncFnOnce(AppConfig) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    consignment_startup(config.consignment.enabled(), config.database_url().is_some())?;
    start(config).await
}

async fn run_configured_service(config: AppConfig) -> anyhow::Result<()> {
    let mut signals = TerminationSignals::subscribe().context("订阅 Worker 终止信号失败")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_worker",
        "worker",
        env!("CARGO_PKG_VERSION"),
        config.log_filter.as_deref(),
        Some(&tracing_config),
        config.tracing_headers(),
    )
    .context("初始化日志与 tracing 失败")?;
    auction_observability::install_safe_panic_hook();

    let mut forced = false;
    let result = run_service(&config, &telemetry, &mut signals, &mut forced).await;
    if !forced {
        auction_observability::shutdown_tracing("worker", telemetry, &mut signals).await;
    }
    result
}

fn consignment_startup(enabled: bool, has_database: bool) -> anyhow::Result<ConsignmentActivation> {
    let activation = consignment_activation(
        enabled,
        CONSIGNMENT_COMPLETE,
        cfg!(feature = "consignment-dev-preview"),
        cfg!(debug_assertions),
    )
    .context("Worker 的一口价挂单配置无效")?;
    if activation == ConsignmentActivation::Enabled && !has_database {
        anyhow::bail!("启用一口价挂单的 Worker 必须配置 PostgreSQL database_url");
    }
    Ok(activation)
}

async fn run_service(
    config: &AppConfig,
    telemetry: &TracingRuntime,
    signals: &mut TerminationSignals,
    forced: &mut bool,
) -> anyhow::Result<()> {
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_worker",
            "worker",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("初始化 Prometheus 指标失败")?,
    );
    // 告警读侧先做影子比对，旧口径继续持有裁决权；分类口径只在
    // 观察到差异为零并由操作者显式切换后生效。
    let switches = auction_application::runtime_switches::RuntimeSwitches::new(
        auction_application::runtime_switches::RuntimeSwitchRole::Worker,
        config.readiness.audit_failure_mode,
        config.readiness.cache_enabled,
    )
    .with_sli_snapshot_baseline(config.metrics.sli_snapshot_enabled)
    .with_mooglemail_poll_isolation_baseline(config.worker.mooglemail.poll_isolation_enabled)
    .with_metrics_reject_forwarded_requests_baseline(config.metrics.reject_forwarded_requests)
    .with_disposition_read_mode_baseline(
        auction_application::operations::DispositionReadMode::Shadow,
    );
    let _metrics_task = config.metrics.loopback_listen_address(config.role).map(|address| {
        TaskGuard::new(auction_http::spawn_metrics_listener(
            address,
            config.role.name(),
            Arc::clone(&metrics),
            switches.clone(),
        ))
    });
    tracing::info!(
        database_configured = config.database_url().is_some(),
        worker_concurrency = config.worker_concurrency(),
        "Worker 配置已加载"
    );

    let shutdown_runtime = shutdown::WorkerShutdown::new(config.shutdown, metrics.shutdown());
    let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel::<()>();
    // 首个运行时故障即可触发收口；固定容量避免并发故障排队或阻塞 Drop 路径。
    let (runtime_failure_sender, mut runtime_failure_receiver) =
        tokio::sync::mpsc::channel::<&'static str>(1);
    let worker = run_worker(
        config,
        telemetry.health_reader(),
        Arc::clone(&metrics),
        switches,
        runtime_failure_sender,
        shutdown_runtime.lifecycle.clone(),
        Box::pin(async move {
            let _ = shutdown_receiver.await;
        }),
    );
    tokio::pin!(worker);
    let runtime_failure = async {
        match runtime_failure_receiver.recv().await {
            Some(reason) => reason,
            // 未启用 control/凭据或启动提前失败时，sender 关闭本身不是运行时故障。
            None => std::future::pending::<&'static str>().await,
        }
    };
    tokio::pin!(runtime_failure);
    let result = tokio::select! {
        result = &mut worker => {
            drop(shutdown_sender);
            result
        }
        reason = &mut runtime_failure => {
            tracing::error!(reason, "Worker 运行时故障，开始排空");
            shutdown_runtime.close();
            let _ = shutdown_sender.send(());
            shutdown_runtime.drain(signals, &mut worker, auction_observability::ShutdownReason::RuntimeFailure, forced).await.context("运行失败后排空 Worker 失败")?;
            if reason == "control_server_stopped" {
                return Err(anyhow::anyhow!("Worker control 服务已停止；需要重启 Worker"));
            }
            if reason == "lot_watch_pm_stopped" {
                return Err(anyhow::anyhow!("lot 关注私信 Worker 已停止；需要重启 Worker"));
            }
            if reason == "persistent_worker_stopped" {
                return Err(anyhow::anyhow!("持久 Worker 已停止；需要重启 Worker"));
            }
            // 即使所有在途任务正常结束也须非零退出，由进程管理器获取新 owner/generation。
            Err(anyhow::anyhow!("凭据运行所有权已被撤销；需要重启 Worker"))
        }
        signal = signals.recv() => {
            let signal = signal.context("接收 Worker 终止信号失败")?;
            tracing::info!(signal = %signal, "Worker 开始排空");
            shutdown_runtime.close();
            let _ = shutdown_sender.send(());
            shutdown_runtime.drain(signals, &mut worker, auction_observability::ShutdownReason::Signal, forced).await
        }
    };
    if result.is_ok() {
        tracing::info!("Worker 已停止");
    }
    result
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: std::time::Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: std::time::Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: std::time::Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}

#[cfg(test)]
mod consignment_tests {
    use std::cell::Cell;

    use auction_config::ConsignmentActivation;

    use super::{consignment_startup, run_config};
    use crate::bootstrap::persistent::consignment_test_config;

    #[tokio::test]
    async fn run_config_checks_consignment_before_claiming_work() {
        let claims = Cell::new(0);
        let result = run_config(consignment_test_config(), async |_| {
            claims.set(claims.get() + 1);
            Ok(())
        })
        .await;
        if cfg!(all(feature = "consignment-dev-preview", debug_assertions)) {
            result.unwrap();
            assert_eq!(claims.get(), 1);
        } else {
            assert_eq!(claims.get(), 0);
            assert!(
                format!("{:#}", result.unwrap_err())
                    .contains("consignment-dev-preview 的本地调试构建")
            );
        }
    }

    #[test]
    fn consignment_startup_requires_capable_artifact_and_database() {
        assert_eq!(consignment_startup(false, false).unwrap(), ConsignmentActivation::Disabled);
        if cfg!(all(feature = "consignment-dev-preview", debug_assertions)) {
            assert_eq!(consignment_startup(true, true).unwrap(), ConsignmentActivation::Enabled);
            let error = consignment_startup(true, false).unwrap_err();
            assert!(error.to_string().contains("必须配置 PostgreSQL"));
        } else {
            let error = consignment_startup(true, true).unwrap_err();
            assert!(format!("{error:#}").contains("consignment-dev-preview 的本地调试构建"));
        }
    }
}
