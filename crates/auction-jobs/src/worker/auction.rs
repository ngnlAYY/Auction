//! Auction 生命周期与结算 owner 的独立持久化循环。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use super::{
    PersistentWorker, merge_worker_result, poll_delay, process_claim, process_settlement_claim,
};

/// 立即处理生命周期和结算批次，随后按旧版 polling+jitter cadence 重试。
pub(super) async fn run_loop<C>(worker: Arc<PersistentWorker<C>>)
where
    C: Clock + 'static,
{
    loop {
        let started = std::time::Instant::now();
        let mut result = run_lifecycle_phase(worker.as_ref()).await;
        let settlement_result = run_settlement_phase(worker.as_ref()).await;
        merge_worker_result(&mut result, settlement_result);
        if result != WorkerResult::Success {
            tracing::warn!(
                "auction worker batch was not fully processed; retrying after poll delay"
            );
        }
        worker.metrics.observe_iteration(WorkerLoop::Auction, result, started.elapsed());

        let wait = poll_delay(worker.auction.poll_interval, worker.auction.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        tokio::time::sleep_until(scheduled_wake).await;
        worker.metrics.observe_schedule_delay(
            WorkerLoop::Auction,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}

async fn run_lifecycle_phase<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let Ok(lease_duration) = ChronoDuration::from_std(worker.auction.lease_duration) else {
        tracing::error!("auction lifecycle lease duration is invalid");
        return WorkerResult::Error;
    };
    let capacity = usize::from(worker.auction.batch_limit);
    let mut processed = 0_usize;
    let mut phase_result = WorkerResult::Success;
    while processed < capacity {
        let wave_limit = usize::from(worker.auction.concurrency).min(capacity - processed);
        let wave_limit = u16::try_from(wave_limit).unwrap_or(worker.auction.concurrency);
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease_duration) else {
            tracing::error!("auction lifecycle lease deadline overflowed");
            phase_result = WorkerResult::Error;
            break;
        };
        let item_started = std::time::Instant::now();
        let claims = match tokio::time::timeout(
            worker.auction.item_timeout,
            worker.schedule.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "auction lifecycle claim wave failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("auction lifecycle claim wave timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        };
        if claims.is_empty() {
            break;
        }
        let exhausted = claims.len() < usize::from(wave_limit);
        processed = processed.saturating_add(claims.len());
        let item_timeout = worker.auction.item_timeout.saturating_sub(item_started.elapsed());

        let mut tasks = JoinSet::new();
        for claim in claims {
            let schedule = Arc::clone(&worker.schedule);
            let lifecycle = Arc::clone(&worker.lifecycle);
            let clock = Arc::clone(&worker.clock);
            tasks.spawn(async move {
                let round_id = claim.round_id;
                match tokio::time::timeout(
                    item_timeout,
                    process_claim(schedule, lifecycle, clock, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(%round_id, "auction lifecycle item timed out");
                        WorkerResult::Error
                    }
                }
            });
        }
        merge_tasks(&mut tasks, &mut phase_result, "auction lifecycle item task join failed").await;
        if exhausted {
            break;
        }
    }
    if phase_result == WorkerResult::Success {
        worker.metrics.observe_batch(WorkerLoop::Auction, processed, capacity);
    }
    phase_result
}

async fn run_settlement_phase<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let (Some(store), Some(commands)) = (&worker.settlement_store, &worker.settlement_commands)
    else {
        return WorkerResult::Success;
    };
    let Ok(lease_duration) = ChronoDuration::from_std(worker.auction.lease_duration) else {
        tracing::error!("auction settlement lease duration is invalid");
        return WorkerResult::Error;
    };
    let capacity = usize::from(worker.auction.batch_limit);
    let mut processed = 0_usize;
    let mut phase_result = WorkerResult::Success;
    while processed < capacity {
        let wave_limit = usize::from(worker.auction.concurrency).min(capacity - processed);
        let wave_limit = u16::try_from(wave_limit).unwrap_or(worker.auction.concurrency);
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease_duration) else {
            tracing::error!("auction settlement lease deadline overflowed");
            phase_result = WorkerResult::Error;
            break;
        };
        let item_started = std::time::Instant::now();
        let claims = match tokio::time::timeout(
            worker.auction.item_timeout,
            store.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "auction settlement claim wave failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("auction settlement claim wave timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        };
        if claims.is_empty() {
            break;
        }
        let exhausted = claims.len() < usize::from(wave_limit);
        processed = processed.saturating_add(claims.len());
        let item_timeout = worker.auction.item_timeout.saturating_sub(item_started.elapsed());

        let mut tasks = JoinSet::new();
        for claim in claims {
            let store = Arc::clone(store);
            let commands = Arc::clone(commands);
            let clock = Arc::clone(&worker.clock);
            tasks.spawn(async move {
                let workflow_id = claim.workflow_id;
                match tokio::time::timeout(
                    item_timeout,
                    process_settlement_claim(store, commands, clock, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(%workflow_id, "auction settlement item timed out");
                        WorkerResult::Error
                    }
                }
            });
        }
        merge_tasks(&mut tasks, &mut phase_result, "auction settlement item task join failed")
            .await;
        if exhausted {
            break;
        }
    }
    if phase_result == WorkerResult::Success {
        worker.metrics.observe_batch(WorkerLoop::Auction, processed, capacity);
    }
    phase_result
}

async fn merge_tasks(
    tasks: &mut JoinSet<WorkerResult>,
    phase_result: &mut WorkerResult,
    join_message: &'static str,
) {
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(item_result) => merge_worker_result(phase_result, item_result),
            Err(error) => {
                tracing::error!(error = %error, "{join_message}");
                *phase_result = WorkerResult::Canceled;
            }
        }
    }
}
