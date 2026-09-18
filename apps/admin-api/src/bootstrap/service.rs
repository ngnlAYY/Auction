//! Admin API 的 tracing、HTTP 服务生命周期和有界 drain。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_observability::{ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DEFAULT_DRAIN_TIMEOUT, DrainOutcome, TaskGuard, TerminationSignals, wait_for_drain,
};
use tokio::sync::oneshot;

use super::{config, listener, state};

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = match config_path {
        Some(path) => AppConfig::load_from_path(Role::Admin, path),
        None => AppConfig::load(Role::Admin),
    }
    .context("load admin-api configuration")?;
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

    let result = run_service(&config, &telemetry).await;
    shutdown_tracing(telemetry).await;
    result
}

async fn run_service(config: &AppConfig, telemetry: &TracingRuntime) -> anyhow::Result<()> {
    let mut signals = TerminationSignals::subscribe().context("subscribe admin-api signals")?;
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
    let bound_listener = listener::bind(config).await.context("bind admin-api listener")?;
    let (state, heartbeat) =
        state::configure_state(config, telemetry.health_reader(), Arc::clone(&metrics))
            .await
            .context("configure admin-api state")?;
    let state = state.with_metrics(metrics).with_public_rules_cache(cache.clone());
    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(state::spawn_heartbeat).map(TaskGuard::new);
    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    let mut heartbeat_task: Option<TaskGuard<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let boundary = state.http_boundary();
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
    let result = match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, drain).await {
        Ok(DrainOutcome::Completed(result)) => admin_server_result(result),
        Ok(DrainOutcome::TimedOut) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!("admin-api drain exceeded 10 seconds"))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
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

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
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
