//! Auction 生命周期与结算 owner 的独立持久化循环。

use std::sync::Arc;

use auction_application::Clock;
use auction_application::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

use super::schedule::run_schedule_bootstrap;
use super::{
    PersistentWorker, merge_worker_result, poll_delay, process_claim, process_settlement_claim,
    record_settlement_failure,
};
use crate::worker::diagnostics;

mod slow_proxy;
mod wakeup;

const SETTLEMENT_FAILURE_RECORD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 立即处理生命周期和结算批次，随后按旧版 polling+jitter cadence 重试；若生命周期、慢速代理或
/// 结算有更早的未来到期时刻，则在该时刻提前醒来，常规间隔只作为等待上限。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::Auction;
    let body = async move {
        loop {
            if worker.control.is_draining() {
                return;
            }
            let started = std::time::Instant::now();
            let mut result = run_schedule_bootstrap(worker.as_ref()).await;
            let lifecycle_result = run_lifecycle_phase(worker.as_ref()).await;
            merge_worker_result(&mut result, lifecycle_result);
            if worker.control.is_draining() {
                return;
            }
            let schedule_result = run_schedule_bootstrap(worker.as_ref()).await;
            merge_worker_result(&mut result, schedule_result);
            let slow_proxy_result = slow_proxy::run_phase(worker.as_ref()).await;
            merge_worker_result(&mut result, slow_proxy_result);
            let settlement_result = run_settlement_phase(worker.as_ref()).await;
            merge_worker_result(&mut result, settlement_result);
            if result != WorkerResult::Success {
                tracing::warn!(
                    "auction worker batch was not fully processed; retrying after poll delay"
                );
            }
            worker.metrics.observe_iteration(WorkerLoop::Auction, result, started.elapsed());

            let poll_wait = poll_delay(worker.auction.poll_interval, worker.auction.jitter);
            let wait = wakeup::next_due_wait(worker.as_ref())
                .await
                .map_or(poll_wait, |due_wait| due_wait.min(poll_wait));
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if super::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            worker.metrics.observe_schedule_delay(
                WorkerLoop::Auction,
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
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
    while processed < capacity && !worker.control.is_draining() {
        let wave_limit = usize::from(worker.auction.concurrency).min(capacity - processed);
        let wave_limit = u16::try_from(wave_limit).unwrap_or(worker.auction.concurrency);
        // 数据库时间读取、领取与任务排队共用预算，避免领取前的查询挂住整个 Auction 循环。
        let item_deadline = tokio::time::Instant::now() + worker.auction.item_timeout;
        let now = match tokio::time::timeout_at(item_deadline, worker.schedule.database_now()).await
        {
            Ok(Ok(now)) if tokio::time::Instant::now() < item_deadline => now,
            diagnostic_outcome @ (Ok(Ok(_)) | Err(..)) => {
                diagnostics::unpersisted_error(
                    "auction_lifecycle",
                    if diagnostic_outcome.is_err() {
                        "read_database_clock_timeout"
                    } else {
                        "read_database_clock_expired"
                    },
                    if diagnostic_outcome.is_err() {
                        SafeErrorKind::TIMEOUT
                    } else {
                        SafeErrorKind::LEASE_EXPIRED
                    },
                    DiagnosticContext::default(),
                );
                drop(diagnostic_outcome);

                phase_result = WorkerResult::Error;
                break;
            }
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "auction_lifecycle",
                    "read_database_clock",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
        };
        let Some(lease_until) = now.checked_add_signed(lease_duration) else {
            tracing::error!("auction lifecycle lease deadline overflowed");
            phase_result = WorkerResult::Error;
            break;
        };
        if worker.control.is_draining() {
            break;
        }
        let claims = match tokio::time::timeout_at(
            item_deadline,
            worker.schedule.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "auction_lifecycle",
                    "claim_wave",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "auction_lifecycle",
                    "claim_wave_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                phase_result = WorkerResult::Error;
                break;
            }
        };
        // 领取等待可能跨过 drain；未开始的 claim 保留原租约，恢复时沿用持久命令身份。
        if claims.is_empty() || worker.control.is_draining() {
            break;
        }
        let exhausted = claims.len() < usize::from(wave_limit);
        processed = processed.saturating_add(claims.len());

        let mut tasks = JoinSet::new();
        for claim in claims {
            let schedule = Arc::clone(&worker.schedule);
            let lifecycle = Arc::clone(&worker.lifecycle);
            let clock = Arc::clone(&worker.clock);
            let control = worker.control.clone();
            tasks.spawn(diagnostics::inherit(async move {
                if control.is_draining() {
                    return WorkerResult::Success;
                }
                let round_id = claim.round_id;
                if tokio::time::Instant::now() >= item_deadline {
                    tracing::warn!(%round_id, "auction lifecycle item expired before execution");
                    return WorkerResult::Error;
                }
                match tokio::time::timeout_at(
                    item_deadline,
                    process_claim(schedule, lifecycle, clock, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "auction_lifecycle",
                            "item_timeout",
                            diagnostics::timeout_kind(&error),
                            DiagnosticContext {
                                round_id: diagnostic_id(&round_id),
                                ..Default::default()
                            },
                        );

                        WorkerResult::Error
                    }
                }
            }));
        }
        merge_tasks(&mut tasks, &mut phase_result, "auction_lifecycle").await;
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
    while processed < capacity && !worker.control.is_draining() {
        let wave_limit = usize::from(worker.auction.concurrency).min(capacity - processed);
        let wave_limit = u16::try_from(wave_limit).unwrap_or(worker.auction.concurrency);
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease_duration) else {
            tracing::error!("auction settlement lease deadline overflowed");
            phase_result = WorkerResult::Error;
            break;
        };
        let item_deadline = tokio::time::Instant::now() + worker.auction.item_timeout;
        let claims = match tokio::time::timeout_at(
            item_deadline,
            store.claim_due(worker.worker_id.clone(), now, lease_until, wave_limit),
        )
        .await
        {
            Ok(Ok(claims)) => claims,
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "settlement",
                    "claim_wave",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "settlement",
                    "claim_wave_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                phase_result = WorkerResult::Error;
                break;
            }
        };
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
            let control = worker.control.clone();
            tasks.spawn(diagnostics::inherit(async move {
                if control.is_draining() {
                    return WorkerResult::Success;
                }
                let workflow_id = claim.workflow_id;
                if tokio::time::Instant::now() >= item_deadline {
                    tracing::warn!(%workflow_id, "auction settlement item expired before execution");
                    return WorkerResult::Error;
                }
                let failure_store = Arc::clone(&store);
                let failure_clock = Arc::clone(&clock);
                let failure_claim = claim.clone();
                match tokio::time::timeout_at(
                    item_deadline,
                    process_settlement_claim(store, commands, clock, claim),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
diagnostics::warn(
"settlement",
"item_timeout",
diagnostics::timeout_kind(&error),
DiagnosticContext { workflow_id: diagnostic_id(&workflow_id), ..Default::default() },
);


                        // 与旧版 `context.WithoutCancel` 的两秒收口一致：业务执行超时后仍要尽力
                        // 在独立有界窗口内原子推进 attempt、retry 时间和审计；若结算其实已提交，
                        // settled 状态 fence 会阻止误写失败进度。
                        if tokio::time::timeout(
                            SETTLEMENT_FAILURE_RECORD_TIMEOUT,
                            record_settlement_failure(
                                failure_store,
                                failure_clock,
                                failure_claim,
                                "timeout",
                                "结算处理超过单项时间预算".to_owned(),
                            ),
                        )
                        .await
                        .is_err()
                        {
                            tracing::error!(
                                %workflow_id,
                                "auction settlement timeout failure record also timed out"
                            );
                        }
                        WorkerResult::Error
                    }
                }
            }));
        }
        merge_tasks(&mut tasks, &mut phase_result, "settlement").await;
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
    kind: &'static str,
) {
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(item_result) => merge_worker_result(phase_result, item_result),
            Err(error) => {
                diagnostics::error(
                    kind,
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
                *phase_result = WorkerResult::Canceled;
            }
        }
    }
}
