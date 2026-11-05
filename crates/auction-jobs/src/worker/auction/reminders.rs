//! W12.1 残留：已提交出价事件的标记/被超提醒处理阶段。
//!
//! 出价 Serializable 事务只追加 `lot_watch_events`；本阶段按 lot 的 `state_version` 顺序处理。
//! 领取使用租约与 `SKIP LOCKED`，崩溃后由过期租约回收；失败事件退避重试且永不删除。
//! 阶段不建立业务状态机，只复用域层已提交事实与既有标记/提醒写入。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::{WorkerLoop, WorkerResult};

use super::super::PersistentWorker;
use crate::worker::diagnostics;

/// 处理租约：只用于回收崩溃进程的 claim，必须显著长于单条事件的处理耗时。
const REMINDER_LEASE_SECONDS: u32 = 60;

pub(super) async fn run_phase<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: auction_application::Clock + 'static,
{
    if worker.control.is_draining() {
        return WorkerResult::Success;
    }
    let Some(store) = &worker.lot_watch_reminders else {
        return WorkerResult::Success;
    };
    let limit = worker.auction.batch_limit;
    let capacity = usize::from(limit);
    match tokio::time::timeout(
        worker.auction.item_timeout,
        store.process_due(worker.worker_id.clone(), REMINDER_LEASE_SECONDS, limit),
    )
    .await
    {
        Ok(Ok(batch)) => {
            worker.metrics.observe_batch(WorkerLoop::Auction, usize::from(batch.claimed), capacity);
            if batch.retried > 0 {
                tracing::warn!(
                    claimed = batch.claimed,
                    completed = batch.completed,
                    retried = batch.retried,
                    "lot watch reminder events were retried"
                );
            }
            WorkerResult::Success
        }
        Ok(Err(error)) => {
            diagnostics::unpersisted_error(
                "lot_watch_reminders",
                "process_due",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            WorkerResult::Error
        }
        Err(error) => {
            diagnostics::unpersisted_error(
                "lot_watch_reminders",
                "process_due_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );
            WorkerResult::Error
        }
    }
}
