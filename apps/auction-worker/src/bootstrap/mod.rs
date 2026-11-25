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
    AppConfig, ConsignmentActivation, Role, WebStackBinary, process_consignment_activation,
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

use persistent::{PersistentRuntime, run_worker};

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
    start: impl AsyncFnOnce(AppConfig, ConsignmentActivation) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let consignment = consignment_startup(
        &config,
        cfg!(feature = "consignment-dev-preview"),
        cfg!(debug_assertions),
    )?;
    start(config, consignment).await
}

async fn run_configured_service(
    config: AppConfig,
    consignment: ConsignmentActivation,
) -> anyhow::Result<()> {
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
    let result = run_service(&config, consignment, &telemetry, &mut signals, &mut forced).await;
    if !forced {
        auction_observability::shutdown_tracing("worker", telemetry, &mut signals).await;
    }
    result
}

/// 不变量 6：与 Public、Admin、server、CLI 同一校验器，制品门之后按 Worker 自身摘要核对页面栈回执
/// （D9 §8），任一不符即拒绝启动。整个进程只在这里判定一次，结果向下传给邮件轮询，下游不再重算。
fn consignment_startup(
    config: &AppConfig,
    preview: bool,
    debug: bool,
) -> anyhow::Result<ConsignmentActivation> {
    let build = auction_observability::process_build_id();
    let activation =
        process_consignment_activation(config, WebStackBinary::Worker, build, preview, debug)
            .context("Worker 的一口价挂单配置无效")?;
    if activation == ConsignmentActivation::Enabled && config.database_url().is_none() {
        anyhow::bail!("启用一口价挂单的 Worker 必须配置 PostgreSQL database_url");
    }
    Ok(activation)
}

async fn run_service(
    config: &AppConfig,
    consignment: ConsignmentActivation,
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
        PersistentRuntime {
            consignment,
            tracing_health: telemetry.health_reader(),
            metrics: Arc::clone(&metrics),
            switches,
            runtime_failure: runtime_failure_sender,
            write_lifecycle: shutdown_runtime.lifecycle.clone(),
            shutdown: Box::pin(async move {
                let _ = shutdown_receiver.await;
            }),
        },
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
            if let Err(drain_error) = shutdown_runtime
                .drain(
                    signals,
                    &mut worker,
                    auction_observability::ShutdownReason::RuntimeFailure,
                    forced,
                )
                .await
            {
                return Err(anyhow::anyhow!(
                    "Worker 运行时故障 `{reason}`；排空失败：{drain_error:#}"
                ));
            }
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
            let (received_signal, signal_error) = match signal {
                Ok(signal) => (Some(signal), None),
                Err(error) => (None, Some(error)),
            };
            if let Some(signal) = received_signal {
                tracing::info!(signal = %signal, "Worker 开始排空");
            } else if let Some(error) = signal_error.as_ref() {
                tracing::warn!(error = %error, "接收 Worker 终止信号失败，仍将排空");
            }
            shutdown_runtime.close();
            let _ = shutdown_sender.send(());
            let drain_result = shutdown_runtime
                .drain(
                    signals,
                    &mut worker,
                    auction_observability::ShutdownReason::Signal,
                    forced,
                )
                .await;
            match (signal_error, drain_result) {
                (Some(signal_error), Ok(())) => {
                    Err(anyhow::Error::new(signal_error).context("接收 Worker 终止信号失败"))
                }
                (Some(signal_error), Err(drain_error)) => Err(anyhow::anyhow!(
                    "接收 Worker 终止信号失败：{signal_error}; Worker 排空也失败：{drain_error:#}"
                )),
                (None, result) => result,
            }
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
    use crate::bootstrap::persistent::{consignment_config_with, consignment_test_config};

    const DEV_PREVIEW_ONLY: &str = "consignment-dev-preview 的本地调试构建";

    /// 启动判定失败即不进入任何领取；通过时把同一次判定结果交给下游，不再重算。
    #[tokio::test]
    async fn run_config_checks_consignment_before_claiming_work() {
        let claims = Cell::new(0);
        let result = run_config(consignment_test_config(), async |_, consignment| {
            assert_eq!(consignment, ConsignmentActivation::Enabled);
            claims.set(claims.get() + 1);
            Ok(())
        })
        .await;
        if cfg!(all(feature = "consignment-dev-preview", debug_assertions)) {
            result.unwrap();
            assert_eq!(claims.get(), 1);
        } else {
            assert_eq!(claims.get(), 0);
            assert!(format!("{:#}", result.unwrap_err()).contains(DEV_PREVIEW_ONLY));
        }
    }

    /// 不变量 6：发布构型（非“预览 + 调试”）开启寄售一律失败关闭；只有本地调试预览免回执。
    #[test]
    fn consignment_startup_fails_closed_outside_the_debug_preview() {
        let enabled = consignment_test_config();
        assert_eq!(
            consignment_startup(&enabled, true, true).unwrap(),
            ConsignmentActivation::Enabled
        );
        for (preview, debug) in [(false, false), (true, false), (false, true)] {
            let error = consignment_startup(&enabled, preview, debug).unwrap_err();
            assert!(format!("{error:#}").contains(DEV_PREVIEW_ONLY), "{preview}/{debug}");
        }
        let error =
            consignment_startup(&consignment_config_with(true, false), true, true).unwrap_err();
        assert!(error.to_string().contains("必须配置 PostgreSQL"));
        assert_eq!(
            consignment_startup(&consignment_config_with(false, false), false, false).unwrap(),
            ConsignmentActivation::Disabled
        );
    }
}
