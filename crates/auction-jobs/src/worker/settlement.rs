//! Settlement fulfillment 的独立持久化领取循环。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use crate::worker::diagnostics;
use crate::worker::runtime::{
    PersistentWorker, merge_worker_result, poll_delay, process_fulfillment_claim,
};

/// 立即处理一个批次，随后按旧版 polling+jitter cadence 重试。
/// 批次或单项失败只影响 fulfillment owner；只有循环任务 panic 或意外退出才由装配根
/// 将整个 Worker 标记为 degraded 并失败关闭。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::AuctionFulfillment;
    let body = async move {
        loop {
            if worker.control.is_draining() {
                return;
            }
            // 履约批次包含 Sent 证据读取与 compose 写入；暂停期间不领取新批次，
            // 已领取项按其自身 lease 与 sending 围栏完成。
            if worker.external_access_paused() {
                if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                    return;
                }
                continue;
            }
            let started = std::time::Instant::now();
            let result = run_batch(worker.as_ref()).await;
            if result != WorkerResult::Success {
                tracing::warn!(
                    "fulfillment worker batch was not fully processed; retrying after poll delay"
                );
            }
            worker.metrics.observe_iteration(
                WorkerLoop::AuctionFulfillment,
                result,
                started.elapsed(),
            );

            let wait = poll_delay(worker.fulfillment.poll_interval, worker.fulfillment.jitter);
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if crate::worker::runtime::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            worker.metrics.observe_schedule_delay(
                WorkerLoop::AuctionFulfillment,
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await;
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
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!("fulfillment worker lease duration is invalid");
        return WorkerResult::Error;
    };
    let Ok(retry_delay) = ChronoDuration::from_std(worker.fulfillment.retry_delay) else {
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
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
            diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
            tracing::error!("fulfillment worker lease deadline overflowed");
            batch_result = WorkerResult::Error;
            break;
        };
        // 不把领取后剩余时长交给尚未运行的任务，否则首次 poll 会重置实际截止。
        let item_deadline = tokio::time::Instant::now() + worker.fulfillment.item_timeout;
        let claims = match tokio::time::timeout_at(
            item_deadline,
            store.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "settlement_fulfillment",
                    "claim_wave",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                batch_result = WorkerResult::Error;
                break;
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "settlement_fulfillment",
                    "claim_wave_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                batch_result = WorkerResult::Error;
                break;
            }
        };
        // 领取回执跨过 drain 时尚未执行应用命令，保留原 claim 等待租约恢复。
        if claims.is_empty() || worker.control.is_draining() {
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
            let control = worker.control.clone();
            tasks.spawn(diagnostics::inherit(async move {
                if control.is_draining() {
                    return WorkerResult::Success;
                }
                let obligation_id = claim.obligation_id;
                if tokio::time::Instant::now() >= item_deadline {
                    diagnostics::unpersisted(SafeErrorKind::LEASE_EXPIRED);
                    tracing::warn!(%obligation_id, "fulfillment item expired before execution");
                    return WorkerResult::Error;
                }
                match tokio::time::timeout_at(
                    item_deadline,
                    process_fulfillment_claim(store, commands, clock, metrics, retry_delay, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "settlement_fulfillment",
                            "item_timeout",
                            diagnostics::timeout_kind(&error),
                            DiagnosticContext {
                                obligation_id: diagnostic_id(&obligation_id),
                                ..Default::default()
                            },
                        );

                        WorkerResult::Error
                    }
                }
            }));
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(item_result) => merge_worker_result(&mut batch_result, item_result),
                Err(error) => {
                    diagnostics::observe_task_panic("settlement_fulfillment", &error);
                    diagnostics::error(
                        "settlement_fulfillment",
                        "item_join",
                        diagnostics::join_kind(&error),
                        DiagnosticContext {
                            disposition: if error.is_cancelled() {
                                None
                            } else {
                                Some(DiagnosticDisposition::Unpersisted)
                            },
                            ..Default::default()
                        },
                    );
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
