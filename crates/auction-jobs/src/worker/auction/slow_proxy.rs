//! Auction Worker 的慢速代理到期阶段。

use std::sync::Arc;

use auction_application::auction::{SlowProxyCommandPort, SlowProxyDue, is_stale_slow_proxy_error};
use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, diagnostic_id,
};
use auction_observability::{WorkerLoop, WorkerResult};
use tokio::task::JoinSet;

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, merge_worker_result};

/// 每轮只执行一次数据库时间驱动的有界扫描，避免 stale 候选在同轮内被反复读取。
pub(super) async fn run_phase<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    if worker.control.is_draining() {
        return WorkerResult::Success;
    }
    let (Some(reader), Some(commands)) = (&worker.slow_proxy_due, &worker.slow_proxy_commands)
    else {
        return WorkerResult::Success;
    };
    let capacity = usize::from(worker.auction.batch_limit);
    let candidates = match tokio::time::timeout(
        worker.auction.item_timeout,
        reader.list_due_slow_proxies(worker.auction.batch_limit),
    )
    .await
    {
        Ok(Ok(candidates)) => candidates,
        Ok(Err(error)) => {
            diagnostics::unpersisted_error(
                "slow_proxy",
                "scan_candidates",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            return WorkerResult::Error;
        }
        Err(error) => {
            diagnostics::unpersisted_error(
                "slow_proxy",
                "scan_candidates_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );

            return WorkerResult::Error;
        }
    };
    worker.metrics.observe_batch(WorkerLoop::Auction, candidates.len(), capacity);

    let mut phase_result = WorkerResult::Success;
    let concurrency = usize::from(worker.auction.concurrency);
    for wave in candidates.chunks(concurrency) {
        if worker.control.is_draining() {
            break;
        }
        let mut tasks = JoinSet::new();
        for due in wave.iter().copied() {
            let commands = Arc::clone(commands);
            let item_timeout = worker.auction.item_timeout;
            let control = worker.control.clone();
            tasks.spawn(diagnostics::inherit(async move {
                if control.is_draining() {
                    return WorkerResult::Success;
                }
                match tokio::time::timeout(item_timeout, execute_candidate(commands, due)).await {
                    Ok(result) => result,
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "slow_proxy",
                            "item_timeout",
                            diagnostics::timeout_kind(&error),
                            DiagnosticContext {
                                authorization_id: diagnostic_id(&due.authorization_id),
                                lot_id: diagnostic_id(&due.lot_id),
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
                Ok(item_result) => merge_worker_result(&mut phase_result, item_result),
                Err(error) => {
                    diagnostics::observe_task_panic("slow_proxy", &error);
                    diagnostics::error(
                        "slow_proxy",
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
                    if error.is_panic() {
                        // 没有持久失败端口的到期读取不能无限回扫同一故障项；交给常驻监管失败关闭。
                        std::panic::resume_unwind(error.into_panic());
                    }
                    phase_result = WorkerResult::Canceled;
                }
            }
        }
    }
    phase_result
}

async fn execute_candidate(
    commands: Arc<dyn SlowProxyCommandPort>,
    due: SlowProxyDue,
) -> WorkerResult {
    let command = match due.into_command() {
        Ok(command) => command,
        Err(error) => {
            diagnostics::unpersisted_error(
                "slow_proxy",
                "build_command",
                error.diagnostic_kind(),
                DiagnosticContext {
                    lot_id: diagnostic_id(&due.lot_id),
                    authorization_id: diagnostic_id(&due.authorization_id),
                    ..Default::default()
                },
            );
            return WorkerResult::Error;
        }
    };
    match commands.execute_slow_proxy(command).await {
        Ok(_) => {
            tracing::info!(
                lot_id = %due.lot_id,
                authorization_id = %due.authorization_id,
                command_id = %command.command_id,
                generation = due.generation,
                due_at = %due.due_at,
                "auction slow proxy candidate processed"
            );
            WorkerResult::Success
        }
        Err(error) if is_stale_slow_proxy_error(&error) => {
            tracing::warn!(
                lot_id = %due.lot_id,
                authorization_id = %due.authorization_id,
                generation = due.generation,
                due_at = %due.due_at,
                error_category = "stale_candidate",
                "auction slow proxy candidate is stale"
            );
            WorkerResult::Success
        }
        Err(error) => {
            diagnostics::unpersisted_error(
                "slow_proxy",
                "execute_command",
                error.diagnostic_kind(),
                DiagnosticContext {
                    lot_id: diagnostic_id(&due.lot_id),
                    command_id: diagnostic_id(&command.command_id),
                    authorization_id: diagnostic_id(&due.authorization_id),
                    ..Default::default()
                },
            );
            WorkerResult::Error
        }
    }
}
