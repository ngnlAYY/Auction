//! Admin API 的 tracing、HTTP 服务生命周期和有界 drain。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_observability::{ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain};
use tokio::sync::oneshot;

use super::{config, listener, state};

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = match config_path {
        Some(path) => AppConfig::load_from_path(Role::Admin, path),
        None => AppConfig::load(Role::Admin),
    }
    .context("load admin-api configuration")?;
    let mut signals = TerminationSignals::subscribe().context("subscribe admin-api signals")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_admin_api",
        "admin",
        env!("CARGO_PKG_VERSION"),
        config.log_filter.as_deref(),
        Some(&tracing_config),
        config.tracing_headers(),
    )
    .context("initialize tracing")?;

    let mut forced = false;
    let result = run_service(&config, &telemetry, &mut signals, &mut forced).await;
    if !forced {
        shutdown_tracing(telemetry, &mut signals).await;
    }
    result
}

async fn run_service(
    config: &AppConfig,
    telemetry: &TracingRuntime,
    signals: &mut TerminationSignals,
    forced: &mut bool,
) -> anyhow::Result<()> {
    let cache = Arc::new(config::configure_cache(config).context("configure admin-api cache")?);
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_admin_api",
            "admin",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    let shutdown_metrics = metrics.shutdown();
    // 依赖装配可能等待数据库锁或连接；终止信号不能等到开始提供 HTTP 后才被读取。
    let (bound_listener, state, heartbeat) = tokio::select! {
        biased;
        signal = signals.recv() => {
            let signal = signal.context("receive admin-api startup signal")?;
            anyhow::bail!("admin-api startup interrupted by {signal}");
        }
        result = async {
            let bound_listener = listener::bind(config).await.context("bind admin-api listener")?;
            let (state, heartbeat) =
                state::configure_state(config, telemetry.health_reader(), Arc::clone(&metrics))
                    .await
                    .context("configure admin-api state")?;
            Ok::<_, anyhow::Error>((bound_listener, state, heartbeat))
        } => result?,
    };
    let state = state.with_metrics(metrics).with_public_rules_cache(cache.clone());
    #[cfg(feature = "postgres")]
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(state::spawn_heartbeat).map(TaskGuard::new);
    #[cfg(not(feature = "postgres"))]
    let mut heartbeat_task: Option<TaskGuard<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let boundary =
        state.http_boundary().with_metrics_access(config.metrics.reject_forwarded_requests);
    let app = auction_http::admin_router::<crate::web_assets::AdminWebAssets>(
        env!("CARGO_PKG_VERSION"),
        boundary,
    )
    .with_state(state);

    tracing::info!(
        listener = %bound_listener.description(),
        cache_backend = cache.backend(),
        "admin api listening"
    );
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task =
        TaskGuard::new(tokio::spawn(listener::serve(bound_listener, app, async move {
            let _ = shutdown_receiver.await;
        })));
    let mut immediate_server_result = None;
    let signal = tokio::select! {
        result = server_task.join() => {
            immediate_server_result = result;
            None
        }
        signal = signals.recv() => Some(signal.context("receive admin-api shutdown signal")?),
    };

    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "admin api drain started");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    abort_heartbeat_task(&heartbeat_task);
    let drain_started = std::time::Instant::now();
    let reason = if signal.is_some() {
        auction_observability::ShutdownReason::Signal
    } else {
        auction_observability::ShutdownReason::ServerStopped
    };
    let drain = async {
        join_heartbeat_task(&mut heartbeat_task).await;
        if let Some(publisher) = heartbeat.as_ref()
            && let Err(error) = publisher.publish_stopping().await
        {
            tracing::warn!(error = %error, "admin heartbeat stopping publish failed");
        }
        match immediate_server_result.take() {
            Some(result) => Some(result),
            None => server_task.join().await,
        }
    };
    let outcome =
        wait_for_drain(signals, Duration::from_secs(config.shutdown.drain_timeout_seconds), drain)
            .await;
    shutdown_metrics.phase_finished(
        auction_observability::ShutdownRole::Admin,
        auction_observability::ShutdownPhase::Drain,
        drain_started.elapsed(),
    );
    let label = match &outcome {
        Ok(DrainOutcome::Completed(Some(Ok(Ok(()))))) => {
            auction_observability::ShutdownOutcome::Completed
        }
        Ok(DrainOutcome::Completed(_result)) => auction_observability::ShutdownOutcome::Failed,
        Err(error) => {
            tracing::warn!(error = %error, "排空信号接收失败");
            auction_observability::ShutdownOutcome::Failed
        }
        Ok(DrainOutcome::TimedOut) => auction_observability::ShutdownOutcome::TimedOut,
        Ok(DrainOutcome::Interrupted(_)) => auction_observability::ShutdownOutcome::Interrupted,
    };
    shutdown_metrics.finished(auction_observability::ShutdownRole::Admin, reason, label);
    let result = match outcome {
        Ok(DrainOutcome::Completed(result)) => admin_server_result(result),
        Ok(DrainOutcome::TimedOut) => {
            *forced = true;
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!(
                "admin-api drain exceeded {} seconds",
                config.shutdown.drain_timeout_seconds
            ))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            *forced = true;
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!("admin-api drain interrupted by {signal}"))
        }
        Err(error) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(error).context("wait for admin-api drain")
        }
    };
    if result.is_ok() {
        tracing::info!("admin api stopped");
    }
    result
}

fn abort_heartbeat_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_heartbeat_task(task: &mut Option<TaskGuard<()>>) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(error = %error, "admin heartbeat task join failed");
    }
}

async fn force_admin_tasks(
    heartbeat_task: &mut Option<TaskGuard<()>>,
    server_task: &mut TaskGuard<std::io::Result<()>>,
) {
    abort_heartbeat_task(heartbeat_task);
    server_task.abort();
    join_heartbeat_task(heartbeat_task).await;
    let _ = server_task.join().await;
}

fn admin_server_result(
    result: Option<Result<std::io::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(error).context("serve admin-api HTTP"),
        Some(Err(error)) => Err(error).context("join admin-api HTTP server"),
        None => Err(anyhow::anyhow!("admin-api HTTP server result is missing")),
    }
}

async fn shutdown_tracing(telemetry: TracingRuntime, signals: &mut TerminationSignals) {
    let result = tokio::select! {
        biased;
        signal = signals.recv() => {
            // 业务 owner 已完成回收；spawn_blocking 的刷新无法靠丢弃 future 中断。
            // 二次信号直接结束角色进程，不能让可选 exporter 拖延强制退出。
            match signal {
                Ok(signal) => tracing::warn!(signal = %signal, "tracing 刷新被终止信号强制结束"),
                Err(error) => tracing::warn!(error = %error, "tracing 刷新期间信号接收失败"),
            }
            std::process::exit(1);
        }
        result = tokio::task::spawn_blocking(move || telemetry.shutdown()) => result,
    };
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "admin tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "admin tracing shutdown task failed"),
    }
}

fn tracing_config(settings: &auction_config::TracingSettings) -> TracingConfig {
    TracingConfig {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        batch_timeout: Duration::from_secs(settings.batch_timeout_seconds),
        export_timeout: Duration::from_secs(settings.export_timeout_seconds),
        queue_capacity: settings.queue_capacity as usize,
        max_export_batch_size: settings.max_export_batch_size as usize,
        sample_ratio: f64::from(settings.sample_ratio_ppm) / 1_000_000.0,
        shutdown_timeout: Duration::from_secs(settings.shutdown_timeout_seconds),
    }
}
