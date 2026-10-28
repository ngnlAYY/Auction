//! Auction Worker 的慢速代理到期阶段。

use std::sync::Arc;

use auction_application::{Clock, SlowProxyCommandPort, SlowProxyDue, is_stale_slow_proxy_error};
use auction_observability::{WorkerLoop, WorkerResult};
use tokio::task::JoinSet;

use super::super::{PersistentWorker, merge_worker_result};

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
            tracing::warn!(error = %error, "auction slow proxy scan failed");
            return WorkerResult::Error;
        }
        Err(_) => {
            tracing::warn!("auction slow proxy scan timed out");
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
            tasks.spawn(async move {
                if control.is_draining() {
                    return WorkerResult::Success;
                }
                match tokio::time::timeout(item_timeout, execute_candidate(commands, due)).await {
                    Ok(result) => result,
                    Err(_) => {
                        tracing::warn!(
                            lot_id = %due.lot_id,
                            authorization_id = %due.authorization_id,
                            generation = due.generation,
                            "auction slow proxy item timed out"
                        );
                        WorkerResult::Error
                    }
                }
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(item_result) => merge_worker_result(&mut phase_result, item_result),
                Err(error) => {
                    tracing::error!(error = %error, "auction slow proxy item task join failed");
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
            tracing::warn!(
                lot_id = %due.lot_id,
                authorization_id = %due.authorization_id,
                generation = due.generation,
                error = %error,
                "auction slow proxy candidate is invalid"
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
            tracing::warn!(
                lot_id = %due.lot_id,
                authorization_id = %due.authorization_id,
                command_id = %command.command_id,
                generation = due.generation,
                error = %error,
                "auction slow proxy candidate failed"
            );
            WorkerResult::Error
        }
    }
}
