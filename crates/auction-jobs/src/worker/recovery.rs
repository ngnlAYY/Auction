//! Settlement recovery 的独立五阶段持久化循环。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult, WorkerRetryKind};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use super::{PersistentWorker, merge_worker_result, poll_delay, process_recovery_action};

/// 立即依次推进 health、case、return、action、relist 与 refund，随后按旧版 cadence 重试。
pub(super) async fn run_loop<C>(worker: Arc<PersistentWorker<C>>)
where
    C: Clock + 'static,
{
    loop {
        let started = std::time::Instant::now();
        let result = run_batch(worker.as_ref()).await;
        if result != WorkerResult::Success {
            tracing::warn!(
                "recovery worker batch was not fully processed; retrying after poll delay"
            );
        }
        worker.metrics.observe_iteration(WorkerLoop::AuctionRecovery, result, started.elapsed());

        let wait = poll_delay(worker.recovery.poll_interval, worker.recovery.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        tokio::time::sleep_until(scheduled_wake).await;
        worker.metrics.observe_schedule_delay(
            WorkerLoop::AuctionRecovery,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}

async fn run_batch<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let mut batch_result = WorkerResult::Success;
    refresh_health(worker, &mut batch_result).await;
    advance_cases(worker, &mut batch_result).await;
    record_returns(worker, &mut batch_result).await;
    process_actions(worker, &mut batch_result).await;
    advance_relists(worker, &mut batch_result).await;
    complete_refunds(worker, &mut batch_result).await;
    batch_result
}

async fn refresh_health<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(health) = &worker.recovery_health else {
        return;
    };
    match tokio::time::timeout(worker.recovery.item_timeout, health.run_once(worker.clock.now()))
        .await
    {
        Ok(Ok(true)) => tracing::info!("recovery health projection refreshed"),
        Ok(Ok(false)) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "recovery health refresh failed");
            merge_worker_result(batch_result, WorkerResult::Error);
        }
        Err(_) => {
            tracing::warn!("recovery health refresh timed out");
            merge_worker_result(batch_result, WorkerResult::Error);
        }
    }
}

async fn advance_cases<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.advance_due_case(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    reminder_count = advance.reminder_count,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery case advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery case phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery case item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

async fn record_returns<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.record_due_return(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(recorded))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %recorded.recovery_id,
                    return_mail_id = %recorded.return_mail_id,
                    stage = %recorded.stage,
                    blocking_code = ?recorded.blocking_code,
                    action_kinds = ?recorded.action_kinds,
                    state_version = recorded.state_version,
                    "recovery return evidence recorded"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery return phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery return item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

async fn process_actions<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
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

async fn advance_relists<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_resolution else {
        return;
    };
    let Ok(retry_delay) = ChronoDuration::from_std(worker.recovery.retry_delay) else {
        tracing::error!("recovery relist retry delay is invalid");
        merge_worker_result(batch_result, WorkerResult::Error);
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.advance_due_relist(worker.clock.now(), retry_delay),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    resolution = ?advance.resolution,
                    relisted_auction_id = ?advance.relisted_auction_id,
                    relisted_lot_id = ?advance.relisted_lot_id,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery relist resolution advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery relist phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery relist item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

async fn complete_refunds<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_resolution else {
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.complete_due_refund(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    resolution = ?advance.resolution,
                    relisted_auction_id = ?advance.relisted_auction_id,
                    relisted_lot_id = ?advance.relisted_lot_id,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery refund resolution advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery refund phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery refund item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

fn observe_phase<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
    phase_result: WorkerResult,
    count: usize,
) where
    C: Clock + 'static,
{
    if phase_result == WorkerResult::Success {
        worker.metrics.observe_batch(
            WorkerLoop::AuctionRecovery,
            count,
            usize::from(worker.recovery.batch_limit),
        );
    }
    merge_worker_result(batch_result, phase_result);
}
