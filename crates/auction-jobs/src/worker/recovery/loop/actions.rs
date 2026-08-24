//! Recovery action claim、并发处理和 lease 收口阶段。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult, WorkerRetryKind};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use super::{PersistentWorker, merge_worker_result, observe_phase, process_recovery_action};

pub(super) async fn process_actions<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    if worker.recovery_delivery.is_none() && worker.recovery_blacklister.is_none() {
        return;
    }
    let Ok(lease_duration) = ChronoDuration::from_std(worker.recovery.lease_duration) else {
        tracing::error!("recovery action lease duration is invalid");
        merge_worker_result(batch_result, WorkerResult::Error);
        return;
    };
    let Ok(retry_delay) = ChronoDuration::from_std(worker.recovery.retry_delay) else {
        tracing::error!("recovery action retry delay is invalid");
        merge_worker_result(batch_result, WorkerResult::Error);
        return;
    };

    let capacity = usize::from(worker.recovery.batch_limit);
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    let mut remaining = capacity;
    let mut exhausted = false;
    while remaining > 0 && !exhausted {
        let wave_size = usize::from(worker.recovery.concurrency).min(remaining);
        let mut tasks = JoinSet::new();
        for _ in 0..wave_size {
            let item_started = std::time::Instant::now();
            let now = worker.clock.now();
            let Some(lease_until) = now.checked_add_signed(lease_duration) else {
                tracing::error!("recovery action lease deadline overflowed");
                phase_result = WorkerResult::Error;
                exhausted = true;
                break;
            };
            let claim = match tokio::time::timeout(
                worker.recovery.item_timeout,
                store.claim_due_action(worker.worker_id.clone(), now, lease_until),
            )
            .await
            {
                Ok(Ok(Some(claim))) => claim,
                Ok(Ok(None)) => {
                    exhausted = true;
                    break;
                }
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "recovery action claim phase failed");
                    phase_result = WorkerResult::Error;
                    exhausted = true;
                    break;
                }
                Err(_) => {
                    tracing::warn!("recovery action claim timed out");
                    phase_result = WorkerResult::Error;
                    exhausted = true;
                    break;
                }
            };
            remaining = remaining.saturating_sub(1);
            count = count.saturating_add(1);
            if claim.retry_count > 0 {
                worker
                    .metrics
                    .observe_retry(WorkerLoop::AuctionRecovery, WorkerRetryKind::Persisted);
            }
            let store = Arc::clone(store);
            let delivery = worker.recovery_delivery.clone();
            let blacklister = worker.recovery_blacklister.clone();
            let clock = Arc::clone(&worker.clock);
            let item_timeout = worker.recovery.item_timeout.saturating_sub(item_started.elapsed());
            tasks.spawn(async move {
                let action_id = claim.action_id;
                match tokio::time::timeout(
                    item_timeout,
                    process_recovery_action(
                        store,
                        delivery,
                        blacklister,
                        clock,
                        retry_delay,
                        claim,
                    ),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(%action_id, "recovery action item timed out");
                        WorkerResult::Error
                    }
                }
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(action_result) => merge_worker_result(&mut phase_result, action_result),
                Err(error) => {
                    tracing::error!(error = %error, "recovery action task join failed");
                    phase_result = WorkerResult::Canceled;
                }
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}
