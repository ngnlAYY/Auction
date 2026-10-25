//! Public API 的 tracing、HTTP 服务生命周期和有界 drain。

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

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load public-api configuration")?;
    run_config(config).await
}

/// 从显式角色路径启动，不受父进程配置环境变量影响。
pub async fn run_from_path(path: std::path::PathBuf) -> anyhow::Result<()> {
    let config = AppConfig::load_from_path(Role::Public, path)
        .context("load explicit public-api configuration")?;
    run_config(config).await
}

async fn run_config(config: AppConfig) -> anyhow::Result<()> {
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_public_api",
        "public",
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
    let mut signals = TerminationSignals::subscribe().context("subscribe public-api signals")?;
    let cache = Arc::new(config::configure_cache(config).context("configure public-api cache")?);
    let metrics = Arc::new(
        ProcessMetrics::new_with_tracing_health(
            config.metrics.enabled,
            "auction_public_api",
            "public",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("initialize Prometheus metrics")?,
    );
    // 订阅信号后也必须轮询启动等待；取消时 listener 与未完成装配一起释放，尚无后台任务。
    let (listener, state, heartbeat, realtime_relay) = tokio::select! {
        biased;
        signal = signals.recv() => {
            let signal = signal.context("receive public-api startup signal")?;
            anyhow::bail!("public-api startup interrupted by {signal}");
        }
        result = async {
            let listener = listener::bind(config).await?;
            let (state, heartbeat, realtime_relay) = state::configure_state(
                config,
                telemetry.health_reader(),
                Arc::clone(&metrics),
                cache.clone(),
            )
            .await
            .context("configure public-api state")?;
            Ok::<_, anyhow::Error>((listener, state, heartbeat, realtime_relay))
        } => result?,
    };
    let listener_description = listener.description();
    #[cfg(not(feature = "postgres"))]
    let _ = realtime_relay;
    let state = state.with_registration_info(config.bot_players.clone()).with_metrics(metrics);
    let realtime_shutdown_state = state.clone();
    #[cfg(feature = "postgres")]
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(state::spawn_heartbeat).map(TaskGuard::new);
    #[cfg(not(feature = "postgres"))]
    let mut heartbeat_task: Option<TaskGuard<()>> = None;
    #[cfg(feature = "postgres")]
    let mut realtime_relay_task = realtime_relay
        .map(|relay| {
            state::spawn_realtime_relay(
                relay,
                std::time::Duration::from_millis(config.realtime.poll_interval_millis),
            )
        })
        .map(TaskGuard::new);
    #[cfg(not(feature = "postgres"))]
    let mut realtime_relay_task: Option<TaskGuard<()>> = None;
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let boundary = state.http_boundary();
    let app = auction_http::public_router::<crate::web_assets::PublicWebAssets>(
        env!("CARGO_PKG_VERSION"),
        boundary,
    )
    .with_state(state);

    tracing::info!(listener = %listener_description, cache_backend = cache.backend(), "public api listening");
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task = TaskGuard::new(tokio::spawn(async move {
        listener::serve(listener, app, async move {
            let _ = shutdown_receiver.await;
        })
        .await
    }));
    let mut immediate_server_result = None;
    let signal = tokio::select! {
        result = server_task.join() => {
            immediate_server_result = result;
            None
        }
        signal = signals.recv() => Some(signal.context("receive public-api shutdown signal")?),
    };

    // HTTP server 自行退出时也须关闭已升级连接，不能只依靠 listener graceful shutdown。
    realtime_shutdown_state.begin_realtime_shutdown();
    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "public api drain started");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    abort_background_task(&heartbeat_task);
    abort_background_task(&realtime_relay_task);

    let drain = async {
        join_background_task(&mut heartbeat_task, "public heartbeat").await;
        join_background_task(&mut realtime_relay_task, "public realtime relay").await;
        if let Some(publisher) = heartbeat.as_ref()
            && let Err(error) = publisher.publish_stopping().await
        {
            tracing::warn!(error = %error, "public heartbeat stopping publish failed");
        }
        let server_result = match immediate_server_result.take() {
            Some(result) => Some(result),
            None => server_task.join().await,
        };
        let realtime_result = realtime_shutdown_state
            .drain_realtime_connections()
            .await
            .context("drain public realtime connections");
        match (public_server_result(server_result), realtime_result) {
            (Ok(()), result) => result,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(realtime_error)) => Err(error.context(realtime_error)),
        }
    };
    let result = match wait_for_drain(&mut signals, DEFAULT_DRAIN_TIMEOUT, drain).await {
        Ok(DrainOutcome::Completed(result)) => result,
        Ok(DrainOutcome::TimedOut) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api drain exceeded 10 seconds"))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api drain interrupted by {signal}"))
        }
        Err(error) => {
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(error).context("wait for public-api drain")
        }
    };
    if result.is_ok() {
        tracing::info!("public api stopped");
    }
    result
}

fn abort_background_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_background_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(task = name, error = %error, "public background task join failed");
    }
}

async fn force_public_tasks(
    heartbeat_task: &mut Option<TaskGuard<()>>,
    realtime_relay_task: &mut Option<TaskGuard<()>>,
    server_task: &mut TaskGuard<std::io::Result<()>>,
) {
    abort_background_task(heartbeat_task);
    abort_background_task(realtime_relay_task);
    server_task.abort();
    join_background_task(heartbeat_task, "public heartbeat").await;
    join_background_task(realtime_relay_task, "public realtime relay").await;
    let _ = server_task.join().await;
}

fn public_server_result(
    result: Option<Result<std::io::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(error).context("serve public-api HTTP"),
        Some(Err(error)) => Err(error).context("join public-api HTTP server"),
        None => Err(anyhow::anyhow!("public-api HTTP server result is missing")),
    }
}

async fn shutdown_tracing(telemetry: TracingRuntime) {
    match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(error = %error, "public tracing shutdown failed"),
        Err(error) => tracing::warn!(error = %error, "public tracing shutdown task failed"),
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
