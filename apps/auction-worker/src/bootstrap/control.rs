//! Worker control socket、后台任务回收与 heartbeat 辅助装配。

use super::*;

/// control 任务退出即撤销 readiness；panic、abort 和首轮 poll 前取消也必须触发同一收口。
#[cfg(unix)]
pub(super) struct WorkerControlTaskExit {
    control: WorkerControlHandle,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
}

#[cfg(unix)]
impl WorkerControlTaskExit {
    pub(super) fn new(
        control: WorkerControlHandle,
        runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    ) -> Self {
        Self { control, runtime_failure }
    }
}

#[cfg(unix)]
impl Drop for WorkerControlTaskExit {
    fn drop(&mut self) {
        self.control.mark_draining();
        let _ = self.runtime_failure.try_send("control_server_stopped");
    }
}

#[cfg(feature = "postgres")]
struct WorkerHeartbeatHealth {
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
}

#[cfg(feature = "postgres")]
impl HeartbeatHealthReader for WorkerHeartbeatHealth {
    fn readiness(&self) -> auction_observability::HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.control.is_ready() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

#[cfg(feature = "postgres")]
pub(super) fn worker_heartbeat_health(
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
) -> Arc<dyn HeartbeatHealthReader> {
    Arc::new(WorkerHeartbeatHealth { database, control })
}

#[cfg(unix)]
pub(super) fn worker_control_server_config(
    config: &AppConfig,
) -> anyhow::Result<WorkerControlServerConfig> {
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

#[cfg(feature = "postgres")]
pub(super) fn abort_worker_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn join_worker_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        tracing::warn!(task = name, error = %error, "worker background task join failed");
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
            Err(anyhow::Error::msg(error.to_string())).with_context(|| format!("run {name}"))
        }
        Some(Err(error)) => Err(error).with_context(|| format!("join {name}")),
        None => Ok(()),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        // 心跳只描述当前状态；数据库恢复后跳过漏掉的 tick，避免集中重复发布。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "worker heartbeat publish failed");
            }
        }
    })
}
