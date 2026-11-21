//! Worker control socket、后台任务回收与 heartbeat 辅助装配。

mod remote_reads;
pub(super) use remote_reads::WorkerRemoteReads;

use std::sync::Arc;

use anyhow::Context;
use auction_config::AppConfig;
use auction_http::{UnixHttpListener, UnixListenerOptions};
use auction_jobs::{WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig};
use auction_observability::{HeartbeatHealthReader, HeartbeatPublisher, ProcessMetrics};
use auction_process::TaskGuard;
use auction_process::join_error_kind;

/// Worker control socket 的单实例锁文件名，位于 control runtime 目录内；与 Public/Admin
/// listener 的锁名不同，即使目录共享也不会互相占用。
const WORKER_CONTROL_LOCK_FILE_NAME: &str = ".worker-control.lock";

/// 常驻任务退出即撤销 readiness；panic、abort 和首轮 poll 前取消也必须触发同一收口。
pub(super) struct WorkerRuntimeTaskExit {
    control: WorkerControlHandle,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    reason: &'static str,
}

impl WorkerRuntimeTaskExit {
    pub(super) fn new(
        control: WorkerControlHandle,
        runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
        reason: &'static str,
    ) -> Self {
        Self { control, runtime_failure, reason }
    }
}

impl Drop for WorkerRuntimeTaskExit {
    fn drop(&mut self) {
        self.control.mark_draining();
        let _ = self.runtime_failure.try_send(self.reason);
    }
}

struct WorkerHeartbeatHealth {
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
}

impl HeartbeatHealthReader for WorkerHeartbeatHealth {
    fn readiness(&self) -> auction_observability::HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.control.is_ready() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

pub(super) fn worker_heartbeat_health(
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
) -> Arc<dyn HeartbeatHealthReader> {
    Arc::new(WorkerHeartbeatHealth { database, control })
}

fn worker_control_server_config(config: &AppConfig) -> anyhow::Result<WorkerControlServerConfig> {
    let control = &config.worker.control;
    WorkerControlServerConfig::new(
        control.socket_path.clone(),
        control.runtime_directory.clone(),
        control.group.clone(),
        control.allowed_peer_uids.clone(),
        control.allowed_peer_gids.clone(),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))
}

/// 校验 `[worker.control]` 后经 `auction-http` 的 [`UnixHttpListener`] 绑定 socket：runtime
/// 目录链、单实例锁、stale socket 清理、owner/group/权限与退出时按身份删除都由该 listener
/// 统一实现；`auction-jobs` 只在其上叠加 peer allowlist 与控制路由。
pub(super) async fn bind_worker_control_server(
    config: &AppConfig,
    control: WorkerControlHandle,
    metrics: Arc<ProcessMetrics>,
) -> anyhow::Result<WorkerControlServer<UnixHttpListener>> {
    let server_config = worker_control_server_config(config)?;
    let listener = UnixHttpListener::bind(UnixListenerOptions {
        runtime_directory: server_config.runtime_directory.clone(),
        socket_path: server_config.socket_path.clone(),
        group: server_config.group.clone(),
        lock_file_name: WORKER_CONTROL_LOCK_FILE_NAME,
    })
    .await
    .context("绑定 Worker control socket 失败")?;
    Ok(WorkerControlServer::new(listener, server_config, control, Some(metrics)))
}

pub(super) fn abort_worker_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

pub(super) async fn join_worker_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        let kind = join_error_kind(&error);
        tracing::warn!(task = name, kind, "回收 Worker 后台任务失败");
    }
}

pub(super) async fn join_worker_control_task<E>(
    task: &mut Option<TaskGuard<Result<(), E>>>,
    name: &'static str,
) -> anyhow::Result<()>
where
    E: std::fmt::Display,
{
    let Some(task) = task else {
        return Ok(());
    };
    match task.join().await {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => {
            Err(anyhow::Error::msg(error.to_string())).with_context(|| format!("{name} 运行失败"))
        }
        Some(Err(error)) => Err(error).with_context(|| format!("{name} 任务回收失败")),
        None => Ok(()),
    }
}

pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        // 心跳只描述当前状态；数据库恢复后跳过漏掉的 tick，避免集中重复发布。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "发布 Worker 心跳失败");
            }
        }
    })
}

/// 只在 Worker readiness 正常时向外部服务报到；地址只存在于敏感配置与打卡器内。
/// 受限客户端在循环外构造一次，每分钟的打卡复用同一连接池与 TLS 根证书库。
pub(super) fn spawn_external_heartbeat(
    publisher: Arc<HeartbeatPublisher>,
    address: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let heartbeat = auction_integrations::BackupHeartbeat::new(Some(address.as_str()));
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if !publisher.external_heartbeat_ready() {
                continue;
            }
            let result = heartbeat.send().await;
            match result.status {
                auction_integrations::BackupHeartbeatStatus::Sent => {
                    tracing::debug!("Worker 外部心跳已发送");
                }
                auction_integrations::BackupHeartbeatStatus::Failed => {
                    tracing::warn!(http_status = result.http_status, "Worker 外部心跳发送失败");
                }
                auction_integrations::BackupHeartbeatStatus::NotConfigured => return,
            }
        }
    })
}
