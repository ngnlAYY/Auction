//! Admin API 的 tracing、HTTP 服务生命周期和有界 drain。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_observability::{DiagnosticLogOwner, ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DrainOutcome, TaskGuard, TerminationSignals, join_error_kind, wait_for_drain,
};
use tokio::sync::oneshot;

use super::{listener, state};

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = match config_path {
        Some(path) => AppConfig::load_from_path(Role::Admin, path),
        None => AppConfig::load(Role::Admin),
    }
    .context("加载 admin-api 配置失败")?;
    let mut signals = TerminationSignals::subscribe().context("订阅 admin-api 终止信号失败")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_admin_api",
        "admin",
        env!("CARGO_PKG_VERSION"),
        config.log_filter.as_deref(),
        Some(&tracing_config),
        config.tracing_headers(),
    )
    .context("初始化日志与 tracing 失败")?;
    auction_observability::install_safe_panic_hook();

    let mut forced = false;
    let result = {
        let diagnostic_logs = DiagnosticLogOwner::install();
        let body = run_service(&config, &telemetry, &mut signals, &mut forced);
        diagnostic_logs.run(body).await
    };
    if !forced {
        auction_observability::shutdown_tracing("admin", telemetry, &mut signals).await;
    }
    result
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
            "auction_admin_api",
            "admin",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("初始化 Prometheus 指标失败")?,
    );
    let shutdown_metrics = metrics.shutdown();
    let switches = auction_application::runtime_switches::RuntimeSwitches::new(
        auction_application::runtime_switches::RuntimeSwitchRole::Admin,
        config.readiness.audit_failure_mode,
        config.readiness.cache_enabled,
    )
    .with_metrics_reject_forwarded_requests_baseline(config.metrics.reject_forwarded_requests)
    // 告警读侧先影子比对，旧口径继续持有裁决权。
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
    // 依赖装配可能等待数据库锁或连接；终止信号不能等到开始提供 HTTP 后才被读取。
    let (bound_listener, state, heartbeat, refresh_tasks) = tokio::select! {
        biased;
        signal = signals.recv() => {
            let signal = signal.context("启动期间接收 admin-api 信号失败")?;
            anyhow::bail!("admin-api 启动被信号 {signal} 中断");
        }
        result = async {
            let bound_listener = listener::bind(config).await.context("绑定 admin-api 监听失败")?;
            let (state, heartbeat, refresh_tasks) = state::configure_state(
                    config,
                    telemetry.health_reader(),
                    Arc::clone(&metrics),
                    switches.clone(),
                )
                .await
                .context("装配 admin-api 状态失败")?;
            Ok::<_, anyhow::Error>((bound_listener, state, heartbeat, refresh_tasks))
        } => result?,
    };
    let state = state.with_metrics(metrics);
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(state::spawn_heartbeat).map(TaskGuard::new);
    // 传入本二进制的版本，健康检查才能报告运行进程的版本而非契约 crate 的版本。
    let boundary = state.http_boundary();
    let app = auction_http::admin_router::<crate::web_assets::AdminWebAssets>(
        env!("CARGO_PKG_VERSION"),
        boundary,
    )
    .with_state(state);

    tracing::info!(
        listener = %bound_listener.description(),
        "admin-api 开始监听"
    );
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task = TaskGuard::new(tokio::spawn(bound_listener.serve(app, async move {
        let _ = shutdown_receiver.await;
    })));
    let mut immediate_server_result = None;
    let signal = tokio::select! {
        result = server_task.join() => {
            immediate_server_result = result;
            None
        }
        signal = signals.recv() => Some(signal.context("接收 admin-api 终止信号失败")?),
    };

    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "admin-api 开始排空");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    drop(refresh_tasks);
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
            tracing::warn!(error = %error, "发布 Admin stopping 心跳失败");
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
            Err(anyhow::anyhow!("admin-api 排空超过 {} 秒", config.shutdown.drain_timeout_seconds))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            *forced = true;
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(anyhow::anyhow!("admin-api 排空被信号 {signal} 中断"))
        }
        Err(error) => {
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(error).context("等待 admin-api 排空失败")
        }
    };
    if result.is_ok() {
        tracing::info!("admin-api 已停止");
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
        let kind = join_error_kind(&error);
        tracing::warn!(kind, "回收 Admin 心跳任务失败");
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
        Some(Ok(Err(error))) => Err(error).context("admin-api HTTP 服务失败"),
        Some(Err(error)) => Err(error).context("回收 admin-api HTTP 服务任务失败"),
        None => Err(anyhow::anyhow!("admin-api HTTP 服务任务没有结果")),
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
