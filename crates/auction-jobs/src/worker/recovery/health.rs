//! Recovery health projection 阶段。

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::WorkerResult;

use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, merge_worker_result};

pub(super) async fn refresh_health<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(health) = &worker.recovery_health else {
        return;
    };
    // 与资产对账阶段一致：暂停期间不开始新的远端余额读取；投影保持上一轮。
    if worker.external_access_paused() {
        return;
    }
    match tokio::time::timeout(worker.recovery.item_timeout, health.run_once(worker.clock.now()))
        .await
    {
        Ok(Ok(true)) => tracing::info!("recovery health projection refreshed"),
        Ok(Ok(false)) => {}
        Ok(Err(error)) => {
            diagnostics::unpersisted_error(
                "recovery_health",
                "refresh_projection",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            merge_worker_result(batch_result, WorkerResult::Error);
        }
        Err(error) => {
            diagnostics::unpersisted_error(
                "recovery_health",
                "refresh_projection_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );

            merge_worker_result(batch_result, WorkerResult::Error);
        }
    }
}
