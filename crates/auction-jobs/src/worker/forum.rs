//! 论坛发布与回帖导入的独立 Worker 循环；两阶段错误相互隔离，
//! 主题发布失败不跳过回帖导入，也不阻塞其它 Worker 能力。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::{WorkerLoop, WorkerResult};

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, poll_delay};

pub(crate) mod reply;
mod reply_delay;
pub(crate) mod sync;

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
            // W1.9：论坛同步、回帖导入与 PM 都是外部访问；暂停期间不开始新周期。
            if worker.external_access_paused() {
                if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                    return;
                }
                continue;
            }
            let started = std::time::Instant::now();
            // W12.5：论坛同步/回帖/PM 属于批量轮次，受共享连接配额限制；等待期间不占用许可。
            let bulk_permit = worker.bulk_permit().await;
            // 每个循环边界读取一次完整热快照；租约仍来自启动配置。
            let forum = worker.forum_runtime();
            let hot_forum = worker.hot.snapshot().forum;
            let mut cycle_result = WorkerResult::Success;
            let mut cycle_healthy = true;
            if let Some(runtime) = &worker.forum_sync {
                let result = sync::run_cycle(
                    worker.as_ref(),
                    runtime,
                    forum.lease_duration,
                    Some(hot_forum),
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

            let mut earliest_reply_scan = None;
            if let Some(runtime) = &worker.forum_reply {
                let result = reply::run_cycle(
                    worker.as_ref(),
                    runtime,
                    forum.lease_duration,
                    forum.poll_interval,
                    Some(hot_forum),
                )
                .await;
                match result {
                    Ok(hint) => earliest_reply_scan = hint,
                    Err(error) => {
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
            }
            worker.control.mark_forum_cycle(cycle_healthy);
            worker.metrics.observe_iteration(WorkerLoop::Forum, cycle_result, started.elapsed());
            // 本轮数据库工作已结束；等待下一轮期间不占用批量连接配额。
            drop(bulk_permit);

            let mut wait = poll_delay(forum.poll_interval, forum.jitter);
            if let Some(earliest) = earliest_reply_scan {
                // Final 阶段按最早 lot 有效截止排定的导入早于常规轮询；下限 1 秒避免空转。
                wait = wait.min(earliest.max(std::time::Duration::from_secs(1)));
            }
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if crate::worker::runtime::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
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
