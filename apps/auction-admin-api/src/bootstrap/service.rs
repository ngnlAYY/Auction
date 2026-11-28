//! Admin API 的 tracing、HTTP 服务生命周期和有界 drain。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use auction_config::{
    AppConfig, ConsignmentActivation, Role, WebStackBinary, process_consignment_activation,
};
use auction_http::{AdminApiState, SurfacePages};
use auction_observability::{DiagnosticLogOwner, ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DrainOutcome, TaskGuard, TerminationSignals, join_error_kind, wait_for_drain,
};
use axum::Router;
use tokio::sync::oneshot;

use super::{listener, state};

pub async fn run(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = match config_path {
        Some(path) => AppConfig::load_from_path(Role::Admin, path),
        None => AppConfig::load(Role::Admin),
    }
    .context("加载 admin-api 配置失败")?;
    let consignment = startup_preflight(
        &config,
        cfg!(feature = "consignment-dev-preview"),
        cfg!(debug_assertions),
    )?;
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
        let body = run_service(&config, consignment, &telemetry, &mut signals, &mut forced);
        diagnostic_logs.run(body).await
    };
    if !forced {
        auction_observability::shutdown_tracing("admin", telemetry, &mut signals).await;
    }
    result
}

fn startup_preflight(
    config: &AppConfig,
    preview: bool,
    debug: bool,
) -> anyhow::Result<ConsignmentActivation> {
    // 不变量 6：制品门之后核对页面栈回执，回执与本程序不符即拒绝启动（D9 §8）。
    let build = auction_observability::process_build_id();
    process_consignment_activation(config, WebStackBinary::Admin, build, preview, debug)
        .context("校验 Admin 一口价挂单制品能力失败")
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
            "auction_admin_api",
            "admin",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("初始化 Prometheus 指标失败")?,
    );
    let shutdown_metrics = metrics.shutdown();
    // 挂载在启动时一次性加载并核对，失败即拒绝启动，不退回内嵌页面（E1）。
    let pages =
        Arc::new(crate::web_assets::surface_pages(config).context("加载 Admin 静态挂载页面失败")?);
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
                    consignment,
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
    let app = configure_router_with_pages(state, pages);

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
            // 排空信号路径已失效：强制收口后不再复用同一信号等待 tracing 刷新。
            *forced = true;
            force_admin_tasks(&mut heartbeat_task, &mut server_task).await;
            Err(error).context("等待 admin-api 排空失败")
        }
    };
    if result.is_ok() {
        tracing::info!("admin-api 已停止");
    }
    result
}

/// 测试用纯内嵌页面的路由。
#[cfg(test)]
pub(super) fn configure_router(state: AdminApiState) -> Router {
    configure_router_with_pages(
        state,
        Arc::new(SurfacePages::embedded::<crate::web_assets::AdminWebAssets>(
            crate::web_assets::web_script_policy(),
        )),
    )
}

pub(super) fn configure_router_with_pages(
    state: AdminApiState,
    pages: Arc<SurfacePages>,
) -> Router {
    // 健康检查报告运行进程的版本。
    auction_http::admin_router_with_pages(env!("CARGO_PKG_VERSION"), state.http_boundary(), pages)
        .with_state(state)
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

#[cfg(test)]
mod tests {
    use std::fs;

    use auction_config::{AppConfig, ConfigError, Role};
    use uuid::Uuid;

    use super::startup_preflight;

    #[test]
    fn consignment_startup_requires_debug_preview() {
        let enabled = load_consignment_config(true);
        for (preview, debug) in [(false, false), (true, false), (false, true)] {
            let error = startup_preflight(&enabled, preview, debug).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<ConfigError>(),
                Some(ConfigError::ConsignmentPreviewRequired)
            ));
        }
        startup_preflight(&enabled, true, true).unwrap();
        let disabled = load_consignment_config(false);
        startup_preflight(&disabled, false, false).unwrap();
    }

    fn load_consignment_config(enabled: bool) -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("auction-admin-startup-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let role_path = directory.join("admin.toml");
        fs::write(&role_path, "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n")
            .unwrap();
        fs::write(directory.join("common.toml"), format!("[consignment]\nenabled = {enabled}\n"))
            .unwrap();
        fs::write(directory.join("secrets.toml"), "").unwrap();
        let config = AppConfig::load_from_path(Role::Admin, role_path);
        fs::remove_dir_all(directory).unwrap();
        config.unwrap()
    }
}
