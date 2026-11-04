//! 无数据库 Worker 启动骨架与非持久化运行模式。

use super::control::{
    WorkerRuntimeTaskExit, join_worker_control_task, worker_control_server_config,
};
use super::*;

#[cfg(not(feature = "postgres"))]
pub(super) async fn run_worker(
    config: &AppConfig,
    _tracing_health: Arc<dyn TracingHealthReader>,
    metrics: Arc<ProcessMetrics>,
    _switches: auction_application::RuntimeSwitches,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    _write_lifecycle: auction_application::ExternalWriteLifecycle,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if config.database_url().is_some() {
        anyhow::bail!("worker 配置了数据库，但当前制品未编译 postgres feature");
    }
    run_worker_skeleton(config, metrics, runtime_failure, shutdown).await?;
    Ok(())
}

/// 运行没有数据库的启动骨架，并在启用时提供同一套本地 Worker control 观察面。
/// 该路径仍然不领取任务、不伪造数据库就绪；control 只报告进程存活和空的 poller 能力，
/// 让 Admin 在演示/部署检查时可以区分“进程已启动”和“持久化 Worker 已可用”。
#[cfg(unix)]
pub(super) async fn run_worker_skeleton(
    config: &AppConfig,
    metrics: Arc<ProcessMetrics>,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if !config.worker.control.enabled {
        auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
            .await
            .context("run worker skeleton without database")?;
        return Ok(());
    }

    let control = WorkerControlHandle::new(env!("CARGO_PKG_VERSION"), 0);
    control.mark_serving();
    let server_config = worker_control_server_config(config)?;
    let server =
        WorkerControlServer::bind_with_metrics(server_config, control.clone(), Some(metrics))
            .await
            .context("bind Worker control socket for worker skeleton")?;
    let control_shutdown = Arc::new(tokio::sync::Notify::new());
    let control_stop = Arc::clone(&control_shutdown);
    let task_exit =
        WorkerRuntimeTaskExit::new(control.clone(), runtime_failure, "control_server_stopped");
    let mut control_task = Some(TaskGuard::new(tokio::spawn(async move {
        let _task_exit = task_exit;
        server.run(async move { control_stop.notified().await }).await
    })));

    let worker_result = auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
        .await
        .context("run worker skeleton without database");
    control.mark_draining();
    // 启动后立即停机时，控制任务可能尚未开始等待；单接收者通知须保留许可。
    control_shutdown.notify_one();
    let control_result =
        join_worker_control_task(&mut control_task, "skeleton worker control").await;
    worker_result?;
    control_result
}

/// 非 Unix 平台没有本地 UDS control 实现；启用该配置时显式失败，而不是假报可用。
#[cfg(not(unix))]
async fn run_worker_skeleton(
    config: &AppConfig,
    _metrics: Arc<ProcessMetrics>,
    _runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    shutdown: ShutdownFuture,
) -> anyhow::Result<()> {
    if config.worker.control.enabled {
        anyhow::bail!("Worker control 需要 Unix socket，当前平台不支持");
    }
    auction_jobs::run_until_shutdown(config.worker_concurrency(), shutdown)
        .await
        .context("run worker skeleton without database")?;
    Ok(())
}
