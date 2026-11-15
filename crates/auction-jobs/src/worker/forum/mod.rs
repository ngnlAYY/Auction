//! 论坛发布与回帖导入的独立 Worker 循环；两阶段错误相互隔离，
//! 主题发布失败不跳过回帖导入，也不阻塞其它 Worker 能力。

use std::sync::Arc;

use auction_application::admin::WorkerForumHotConfig;
use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::{WorkerLoop, WorkerResult};

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, poll_delay};

pub(crate) mod reply;
mod reply_delay;
pub(crate) mod sync;

/// 论坛 sync 与 reply 共用的单轮批次上限；启动构造与热快照判定使用同一边界。
const MAX_BATCH_SIZE: u16 = 100;

/// 热快照在启动租约下可替换的批次与单项预算；越界或单项预算不小于租约时返回 `None`。
/// sync、reply 的领取与 Worker 热读取的冲突检查共用本判定，claim 永远不能越过租约。
pub(crate) fn hot_claim_budget(
    hot: &WorkerForumHotConfig,
    lease_duration: std::time::Duration,
) -> Option<(u16, std::time::Duration)> {
    let item_timeout = std::time::Duration::from_secs(hot.item_timeout_seconds);
    ((1..=MAX_BATCH_SIZE).contains(&hot.batch_size)
        && !item_timeout.is_zero()
        && item_timeout < lease_duration)
        .then_some((hot.batch_size, item_timeout))
}

pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::Forum;
    let body = async move {
        // Final 排期跨轮保留；只决定唤醒，持久 next_scan_at 仍是领取的唯一依据。
        let mut final_scans = reply::FinalScanSchedule::default();
        loop {
            if worker.control.is_draining() {
                return;
            }
            // 论坛同步与回帖导入都是外部访问；暂停期间不开始新周期。
            if worker.external_access_paused() {
                if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                    return;
                }
                continue;
            }
            let started = std::time::Instant::now();
            // 论坛同步与回帖导入属于批量轮次，受共享连接配额限制。
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
                let outcome = reply::run_cycle(
                    worker.as_ref(),
                    runtime,
                    forum.lease_duration,
                    forum.poll_interval,
                    Some(hot_forum),
                    &mut final_scans,
                )
                .await;
                // 失败只影响健康状态与指标；其他 Final 主题已持久的收紧排期仍须按时唤醒。
                earliest_reply_scan = outcome.earliest_final_scan;
                if let Err(error) = outcome.result {
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
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await;
}
