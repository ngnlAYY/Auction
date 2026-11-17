//! 结算退件案件的 Worker 阶段：激活已提交的持久 Take、收口 Take 终态、收口补寄寄出证明，以及
//! 未绑定退件的只读重新核验。每一步只推进持久事实；外部 Take 由投递循环在租约与发送围栏后执行，
//! 只读核验只读取 Bot 收件箱，不领取、不退回。

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::settlement::SettlementReturnProgress;
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::phase::{observe_phase, run_phase};
use crate::worker::diagnostics;
use crate::worker::runtime::PersistentWorker;

pub(super) async fn advance_settlement_returns<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(store) = &worker.settlement_returns else {
        return;
    };
    run_phase(
        worker,
        batch_result,
        ("activate_settlement_return_take", "activate_settlement_return_take_timeout"),
        |now| store.activate_due_return_take(now),
        log_progress,
    )
    .await;
    run_phase(
        worker,
        batch_result,
        ("settle_settlement_return_take", "settle_settlement_return_take_timeout"),
        |now| store.settle_due_return_take(now),
        log_progress,
    )
    .await;
    run_phase(
        worker,
        batch_result,
        ("complete_settlement_return_resend", "complete_settlement_return_resend_timeout"),
        |now| store.complete_due_return_resend(now),
        log_progress,
    )
    .await;
    recheck_returns(worker, batch_result).await;
}

fn log_progress(progress: &SettlementReturnProgress) {
    tracing::info!(
        return_mail_id = %progress.return_mail_id,
        status = progress.status.as_str(),
        case_version = progress.case_version,
        event_kind = progress.event_kind,
        "结算退件案件已推进"
    );
}

/// 逐项领取待核验案件并只读读取原退件；读取失败保持核验待办，领取租约到期后重试。
async fn recheck_returns<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let (Some(store), Some(reader)) =
        (&worker.settlement_returns, &worker.settlement_return_reader)
    else {
        return;
    };
    let Ok(lease) = ChronoDuration::from_std(worker.recovery.lease_duration) else {
        tracing::error!("结算退件核验租约时长无效");
        observe_phase(worker, batch_result, WorkerResult::Error, 0);
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        if worker.control.is_draining() || worker.external_access_paused() {
            break;
        }
        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(lease) else {
            phase_result = WorkerResult::Error;
            break;
        };
        let item = async {
            let Some(claim) = store.claim_due_return_recheck(now, lease_until).await? else {
                return Ok(None);
            };
            let outcome = reader
                .read_returned_mail(claim.world_id.clone(), claim.external_mail_id.clone())
                .await?;
            store.record_return_recheck(claim, outcome).await.map(Some)
        };
        match tokio::time::timeout(worker.recovery.item_timeout, item).await {
            Ok(Ok(Some(progress))) => {
                count = count.saturating_add(1);
                log_progress(&progress);
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "settlement_return_recheck",
                    "recheck_return",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "settlement_return_recheck",
                    "recheck_return_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}
