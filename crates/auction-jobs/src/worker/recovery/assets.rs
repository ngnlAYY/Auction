//! W11.1–W11.3 资产对账阶段：外部只读观察与报告追加。

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::WorkerResult;

use super::{PersistentWorker, merge_worker_result};
use crate::worker::diagnostics;

/// 到期时执行一次对账；未到期、未注入或外部暂停都直接跳过，不改变其他恢复阶段结果。
pub(super) async fn reconcile_assets<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(reconciliation) = &worker.asset_reconciliation else {
        return;
    };
    // 外部访问已主动暂停时不得开始新的远端读取；报告事实保持上一轮。
    if worker.external_access_paused() {
        return;
    }
    match reconciliation.run_once(worker.clock.now()).await {
        Ok(true) => tracing::info!("asset reconciliation report appended"),
        Ok(false) => {}
        Err(error) => {
            diagnostics::unpersisted_error(
                "asset_reconciliation",
                "run_once",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            merge_worker_result(batch_result, WorkerResult::Error);
        }
    }
}
