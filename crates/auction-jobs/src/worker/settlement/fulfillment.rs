//! Settlement fulfillment 的独立持久化领取循环。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use super::{PersistentWorker, merge_worker_result, poll_delay, process_fulfillment_claim};

/// 立即处理一个批次，随后按旧版 polling+jitter cadence 重试。
///
/// 批次或单项失败只影响 fulfillment owner；只有循环任务 panic 或意外退出才由装配根
/// 将整个 Worker 标记为 degraded 并失败关闭。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        if worker.control.is_draining() {
            return;
        }
        let started = std::time::Instant::now();
        let result = run_batch(worker.as_ref()).await;
        if result != WorkerResult::Success {
            tracing::warn!(
                "fulfillment worker batch was not fully processed; retrying after poll delay"
            );
        }
        worker.metrics.observe_iteration(WorkerLoop::AuctionFulfillment, result, started.elapsed());

        let wait = poll_delay(worker.fulfillment.poll_interval, worker.fulfillment.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        if super::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
        worker.metrics.observe_schedule_delay(
            WorkerLoop::AuctionFulfillment,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}

/// 按并发波次领取，避免批次中尚未开始的任务提前消耗 claim lease。
async fn run_batch<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let (Some(store), Some(commands)) = (&worker.fulfillment_store, &worker.fulfillment_commands)
    else {
        return WorkerResult::Success;
    };
    let Ok(lease_duration) = ChronoDuration::from_std(worker.fulfillment.lease_duration) else {
        tracing::error!("fulfillment worker lease duration is invalid");
        return WorkerResult::Error;
    };
    let Ok(retry_delay) = ChronoDuration::from_std(worker.fulfillment.retry_delay) else {
        tracing::error!("fulfillment worker retry delay is invalid");
        return WorkerResult::Error;
    };

    let capacity = usize::from(worker.fulfillment.batch_limit);
    let mut processed = 0_usize;
    let mut batch_result = WorkerResult::Success;
    while processed < capacity && !worker.control.is_draining() {
        let wave_limit = usize::from(worker.fulfillment.concurrency).min(capacity - processed);
        let wave_limit = u16::try_from(wave_limit).unwrap_or(worker.fulfillment.concurrency);
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease_duration) else {
            tracing::error!("fulfillment worker lease deadline overflowed");
            batch_result = WorkerResult::Error;
            break;
        };
        let item_started = std::time::Instant::now();
        let claims = match tokio::time::timeout(
            worker.fulfillment.item_timeout,
            store.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "fulfillment worker claim wave failed");
                batch_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("fulfillment worker claim wave timed out");
                batch_result = WorkerResult::Error;
                break;
            }
        };
        if claims.is_empty() {
            break;
        }
        let exhausted = claims.len() < usize::from(wave_limit);
        processed = processed.saturating_add(claims.len());

        let mut tasks = JoinSet::new();
        for claim in claims {
            let store = Arc::clone(store);
            let commands = Arc::clone(commands);
            let clock = Arc::clone(&worker.clock);
            let metrics = Arc::clone(&worker.metrics);
            let item_timeout =
                worker.fulfillment.item_timeout.saturating_sub(item_started.elapsed());
            tasks.spawn(async move {
                let obligation_id = claim.obligation_id;
                match tokio::time::timeout(
                    item_timeout,
                    process_fulfillment_claim(store, commands, clock, metrics, retry_delay, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(%obligation_id, "fulfillment worker item timed out");
                        WorkerResult::Error
                    }
                }
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(item_result) => merge_worker_result(&mut batch_result, item_result),
                Err(error) => {
                    tracing::error!(error = %error, "fulfillment worker item task join failed");
                    batch_result = WorkerResult::Canceled;
                }
            }
        }
        if exhausted {
            break;
        }
    }

    if batch_result == WorkerResult::Success {
        worker.metrics.observe_batch(WorkerLoop::AuctionFulfillment, processed, capacity);
    }
    batch_result
}
