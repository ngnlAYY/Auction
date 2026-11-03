//! 论坛发布与回帖导入的独立 Worker 循环；两阶段错误相互隔离，
//! 主题发布失败不跳过回帖导入，也不阻塞其它 Worker 能力。

use std::sync::Arc;

use auction_application::Clock;
use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::{WorkerLoop, WorkerResult};

use super::{PersistentWorker, poll_delay};
use crate::worker::diagnostics;

pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::Forum;
    let body = async move {
        loop {
            if worker.control.is_draining() {
                return;
            }
            let started = std::time::Instant::now();
            let mut cycle_result = WorkerResult::Success;
            let mut cycle_healthy = true;
            if let Some(runtime) = &worker.forum_sync {
                let result = super::forum_sync::run_cycle(
                    worker.as_ref(),
                    runtime,
                    worker.forum.lease_duration,
                )
                .await;
                if let Err(error) = result {
                    cycle_result = WorkerResult::Error;
                    cycle_healthy = false;
                    diagnostics::warn(
                        "forum_sync",
                        "poll_topics",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                }
            }

            if let Some(runtime) = &worker.forum_reply {
                let result = super::forum_reply::run_cycle(
                    worker.as_ref(),
                    runtime,
                    worker.forum.lease_duration,
                    worker.forum.poll_interval,
                )
                .await;
                if let Err(error) = result {
                    cycle_result = WorkerResult::Error;
                    cycle_healthy = false;
                    diagnostics::warn(
                        "forum_reply",
                        "poll_replies",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                }
            }
            worker.control.mark_forum_cycle(cycle_healthy);
            worker.metrics.observe_iteration(WorkerLoop::Forum, cycle_result, started.elapsed());

            let wait = poll_delay(worker.forum.poll_interval, worker.forum.jitter);
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if super::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            worker.metrics.observe_schedule_delay(
                WorkerLoop::Forum,
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
}
