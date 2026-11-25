//! Public API 的 tracing、HTTP 服务生命周期和有界 drain。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use auction_config::{
    AppConfig, ConsignmentActivation, Role, WebStackBinary, process_consignment_activation,
};
use auction_http::{PublicApiState, SurfacePages};
use auction_observability::{DiagnosticLogOwner, ProcessMetrics, TracingConfig, TracingRuntime};
use auction_process::{
    DrainOutcome, TaskGuard, TerminationSignals, join_error_kind, wait_for_drain,
};
use axum::Router;
use tokio::sync::oneshot;

use super::{listener, state};

pub async fn run() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("加载 public-api 配置失败")?;
    run_config(config).await
}

/// 从显式角色路径启动，不受父进程配置环境变量影响。
pub async fn run_from_path(path: std::path::PathBuf) -> anyhow::Result<()> {
    let config = AppConfig::load_from_path(Role::Public, path)
        .context("加载显式指定的 public-api 配置失败")?;
    run_config(config).await
}

async fn run_config(config: AppConfig) -> anyhow::Result<()> {
    let consignment = startup_preflight(
        &config,
        cfg!(feature = "consignment-dev-preview"),
        cfg!(debug_assertions),
    )?;
    let mut signals = TerminationSignals::subscribe().context("订阅 public-api 终止信号失败")?;
    let tracing_config = tracing_config(&config.tracing);
    let telemetry = auction_observability::init_with_config(
        "auction_public_api",
        "public",
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
        auction_observability::shutdown_tracing("public", telemetry, &mut signals).await;
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
    process_consignment_activation(config, WebStackBinary::Public, build, preview, debug)
        .context("校验 Public 一口价挂单制品能力失败")
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
            "auction_public_api",
            "public",
            env!("CARGO_PKG_VERSION"),
            telemetry.health_reader(),
        )
        .context("初始化 Prometheus 指标失败")?,
    );
    let shutdown_metrics = metrics.shutdown();
    // 挂载在启动时一次性加载并核对，失败即拒绝启动，不退回内嵌页面（E1）。
    let pages =
        Arc::new(crate::web_assets::surface_pages(config).context("加载 Public 静态挂载页面失败")?);
    let switches = auction_application::runtime_switches::RuntimeSwitches::new(
        auction_application::runtime_switches::RuntimeSwitchRole::Public,
        config.readiness.audit_failure_mode,
        config.readiness.cache_enabled,
    )
    .with_metrics_reject_forwarded_requests_baseline(config.metrics.reject_forwarded_requests);
    let _metrics_task = config.metrics.loopback_listen_address(config.role).map(|address| {
        TaskGuard::new(auction_http::spawn_metrics_listener(
            address,
            config.role.name(),
            Arc::clone(&metrics),
            switches.clone(),
        ))
    });
    // 订阅信号后也必须轮询启动等待；取消时 listener 与未完成装配一起释放，已建后台任务由 TaskGuard 回收。
    let (listener, state, heartbeat, realtime_relay, refresh_tasks) = tokio::select! {
        biased;
        signal = signals.recv() => {
            let signal = signal.context("启动期间接收 public-api 信号失败")?;
            anyhow::bail!("public-api 启动被信号 {signal} 中断");
        }
        result = async {
            let listener = listener::bind(config).await?;
            let (state, heartbeat, realtime_relay, refresh_tasks) = state::configure_state(
                config,
                consignment,
                telemetry.health_reader(),
                Arc::clone(&metrics),
                switches.clone(),
            )
            .await
            .context("装配 public-api 状态失败")?;
            Ok::<_, anyhow::Error>((listener, state, heartbeat, realtime_relay, refresh_tasks))
        } => result?,
    };
    let listener_description = listener.description();
    let state = state.with_registration_info(config.bot_players.clone()).with_metrics(metrics);
    let realtime_shutdown_state = state.clone();
    let mut heartbeat_task =
        heartbeat.as_ref().map(Arc::clone).map(state::spawn_heartbeat).map(TaskGuard::new);
    let mut realtime_relay_task = realtime_relay
        .map(|relay| {
            state::spawn_realtime_relay(
                relay,
                std::time::Duration::from_millis(config.realtime.poll_interval_millis),
            )
        })
        .map(TaskGuard::new);
    let app = configure_router_with_pages(state, pages);

    tracing::info!(listener = %listener_description, "public-api 开始监听");
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task = TaskGuard::new(tokio::spawn(async move {
        listener
            .serve(app, async move {
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
        signal = signals.recv() => Some(signal.context("接收 public-api 终止信号失败")?),
    };

    // HTTP server 自行退出时也须关闭已升级连接，不能只依靠 listener graceful shutdown。
    realtime_shutdown_state.begin_realtime_shutdown();
    if let Some(signal) = signal {
        tracing::info!(signal = %signal, "public-api 开始排空");
        let _ = shutdown_sender.send(());
    } else {
        drop(shutdown_sender);
    }
    drop(refresh_tasks);
    abort_background_task(&heartbeat_task);
    abort_background_task(&realtime_relay_task);

    let drain_started = std::time::Instant::now();
    let reason = if signal.is_some() {
        auction_observability::ShutdownReason::Signal
    } else {
        auction_observability::ShutdownReason::ServerStopped
    };
    let drain = async {
        let heartbeat_result = join_background_task(&mut heartbeat_task, "public heartbeat").await;
        let relay_result =
            join_background_task(&mut realtime_relay_task, "public realtime relay").await;
        if let Some(publisher) = heartbeat.as_ref()
            && let Err(error) = publisher.publish_stopping().await
        {
            tracing::warn!(error = %error, "发布 Public stopping 心跳失败");
        }
        let server_result = match immediate_server_result.take() {
            Some(result) => Some(result),
            None => server_task.join().await,
        };
        let realtime_result = realtime_shutdown_state
            .drain_realtime_connections()
            .await
            .context("排空 Public 实时连接失败");
        let server_result = public_server_result(server_result);
        let mut errors = Vec::new();
        if let Err(error) = heartbeat_result {
            errors.push(format!("heartbeat task: {error:#}"));
        }
        if let Err(error) = relay_result {
            errors.push(format!("realtime relay task: {error:#}"));
        }
        if let Err(error) = server_result {
            errors.push(format!("HTTP server: {error:#}"));
        }
        if let Err(error) = realtime_result {
            errors.push(format!("realtime connection drain: {error:#}"));
        }
        if errors.is_empty() { Ok(()) } else { Err(anyhow::anyhow!(errors.join("; "))) }
    };
    let outcome =
        wait_for_drain(signals, Duration::from_secs(config.shutdown.drain_timeout_seconds), drain)
            .await;
    shutdown_metrics.phase_finished(
        auction_observability::ShutdownRole::Public,
        auction_observability::ShutdownPhase::Drain,
        drain_started.elapsed(),
    );
    let label = match &outcome {
        Ok(DrainOutcome::Completed(result)) if result.is_ok() => {
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
    shutdown_metrics.finished(auction_observability::ShutdownRole::Public, reason, label);
    let result = match outcome {
        Ok(DrainOutcome::Completed(result)) => result,
        Ok(DrainOutcome::TimedOut) => {
            *forced = true;
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api 排空超过 {} 秒", config.shutdown.drain_timeout_seconds))
        }
        Ok(DrainOutcome::Interrupted(signal)) => {
            *forced = true;
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(anyhow::anyhow!("public-api 排空被信号 {signal} 中断"))
        }
        Err(error) => {
            *forced = true;
            force_public_tasks(&mut heartbeat_task, &mut realtime_relay_task, &mut server_task)
                .await;
            Err(error).context("等待 public-api 排空失败")
        }
    };
    if result.is_ok() {
        tracing::info!("public-api 已停止");
    }
    result
}

/// 测试用纯内嵌页面的路由。
#[cfg(test)]
pub(super) fn configure_router(state: PublicApiState) -> Router {
    configure_router_with_pages(
        state,
        Arc::new(SurfacePages::embedded::<crate::web_assets::PublicWebAssets>(
            crate::web_assets::web_script_policy(),
        )),
    )
}

pub(super) fn configure_router_with_pages(
    state: PublicApiState,
    pages: Arc<SurfacePages>,
) -> Router {
    // 使用运行进程的版本。边界与读闸在注入指标后取得，才能记录请求拒绝。
    auction_http::public_router_with_pages(
        env!("CARGO_PKG_VERSION"),
        state.http_boundary(),
        state.anonymous_read_gate(),
        pages,
    )
    .with_state(state)
}

fn abort_background_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

async fn join_background_task(
    task: &mut Option<TaskGuard<()>>,
    name: &'static str,
) -> anyhow::Result<()> {
    let Some(task) = task else {
        return Ok(());
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        let kind = join_error_kind(&error);
        tracing::warn!(task = name, kind, "回收 Public 后台任务失败");
        return Err(anyhow::Error::new(error).context(format!("回收 {name} 后台任务失败")));
    }
    Ok(())
}

async fn force_public_tasks(
    heartbeat_task: &mut Option<TaskGuard<()>>,
    realtime_relay_task: &mut Option<TaskGuard<()>>,
    server_task: &mut TaskGuard<std::io::Result<()>>,
) {
    abort_background_task(heartbeat_task);
    abort_background_task(realtime_relay_task);
    server_task.abort();
    let heartbeat_result = join_background_task(heartbeat_task, "public heartbeat").await;
    let relay_result = join_background_task(realtime_relay_task, "public realtime relay").await;
    let server_result = server_task.join().await;
    let mut errors = Vec::new();
    if let Err(error) = heartbeat_result {
        errors.push(format!("heartbeat task: {error:#}"));
    }
    if let Err(error) = relay_result {
        errors.push(format!("realtime relay task: {error:#}"));
    }
    if let Some(Err(error)) = server_result
        && !error.is_cancelled()
    {
        errors.push(format!("HTTP server task: {error}"));
    }
    if !errors.is_empty() {
        tracing::warn!(errors = %errors.join("; "), "强制关闭时回收 Public 任务失败");
    }
}
fn public_server_result(
    result: Option<Result<std::io::Result<()>, tokio::task::JoinError>>,
) -> anyhow::Result<()> {
    match result {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => Err(error).context("public-api HTTP 服务失败"),
        Some(Err(error)) => Err(error).context("回收 public-api HTTP 服务任务失败"),
        None => Err(anyhow::anyhow!("public-api HTTP 服务任务没有结果")),
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
    use std::net::SocketAddr;
    use std::sync::Arc;

    use auction_config::{AppConfig, ConfigError, ConsignmentActivation, Role};
    use auction_http::PublicApiState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{configure_router_with_pages, startup_preflight};
    use crate::web_assets::{PublicWebAssets, surface_pages, web_script_policy};

    #[test]
    fn consignment_startup_requires_debug_preview() {
        let enabled = load_config("[consignment]\nenabled = true\n");
        for (preview, debug) in [(false, false), (true, false), (false, true)] {
            let error = startup_preflight(&enabled, preview, debug).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<ConfigError>(),
                Some(ConfigError::ConsignmentPreviewRequired)
            ));
        }
        assert_eq!(
            startup_preflight(&enabled, true, true).unwrap(),
            ConsignmentActivation::Enabled
        );
        let disabled = load_config("[consignment]\nenabled = false\n");
        assert_eq!(
            startup_preflight(&disabled, false, false).unwrap(),
            ConsignmentActivation::Disabled
        );
    }

    /// 未配置挂载根时，装配出的页面与改造前的纯内嵌路由逐字节一致（状态、响应头与正文）。
    #[tokio::test]
    async fn unmounted_pages_answer_byte_for_byte_like_the_embedded_router() {
        let config = load_config("");
        let pages = Arc::new(surface_pages(&config).unwrap());
        let assembled = configure_router_with_pages(PublicApiState::default(), pages);
        let state = PublicApiState::default();
        let embedded = auction_http::public_router_with_web_script_policy::<PublicWebAssets>(
            env!("CARGO_PKG_VERSION"),
            state.http_boundary(),
            state.anonymous_read_gate(),
            web_script_policy(),
        )
        .with_state(state);
        for path in ["/", "/consignment", "/favicon.ico", "/assets/missing.js", "/health"] {
            let response = snapshot(&assembled, path).await;
            // 未配置挂载时不发独立构建号头，与支持挂载之前的响应一致（E3 再补充）。
            assert!(response.1.iter().all(|(name, _)| name != "x-auction-build-independent"));
            assert_eq!(response, snapshot(&embedded, path).await, "{path}");
        }
    }

    /// 配置了挂载根却无法核对（没有入口指针与清单）时返回错误，启动随之失败，不退回内嵌页面。
    #[test]
    fn configured_mount_that_cannot_be_verified_stops_startup() {
        let mount = std::env::temp_dir().join(format!("auction-public-mount-{}", Uuid::new_v4()));
        fs::create_dir_all(mount.join("public")).unwrap();
        fs::write(mount.join("public/ENTRY.v1"), "schema=auction-web-entry/1\n").unwrap();
        let config = load_config(&format!("[web]\nmount_root = '{}'\n", mount.display()));
        let result = surface_pages(&config);
        fs::remove_dir_all(&mount).unwrap();
        assert!(result.is_err());
    }

    async fn snapshot(app: &Router, path: &str) -> (u16, Vec<(String, Vec<u8>)>, Vec<u8>) {
        let mut request = Request::builder()
            .uri(path)
            .header("x-request-id", "0123456789abcdef0123456789abcdef")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 9], 4321))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let mut headers: Vec<_> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
            .collect();
        headers.sort();
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap().to_vec();
        (status, headers, body)
    }

    fn load_config(common: &str) -> AppConfig {
        let directory =
            std::env::temp_dir().join(format!("auction-public-startup-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let role_path = directory.join("public.toml");
        fs::write(&role_path, "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n")
            .unwrap();
        fs::write(directory.join("common.toml"), common).unwrap();
        fs::write(directory.join("secrets.toml"), "").unwrap();
        let config = AppConfig::load_from_path(Role::Public, role_path);
        fs::remove_dir_all(directory).unwrap();
        config.unwrap()
    }
}
