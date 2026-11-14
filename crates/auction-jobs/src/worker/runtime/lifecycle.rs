//! 拍卖生命周期与结算轮次 claim 的应用命令执行；状态迁移仍由应用端口拥有。

use std::sync::Arc;

use auction_application::auction::{
    AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort,
};
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind,
};
use auction_application::core::{Clock, RequestHash};
use auction_application::scheduling::{
    ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore,
};
use auction_observability::WorkerResult;
use serde::Serialize;

use super::common::retry_delay;
use crate::worker::diagnostics;

// panic 不具备可重试的业务错误语义；一次即耗尽自动尝试，保留原 claim fence。
pub(crate) async fn process_claim<C: Clock + 'static>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    claim: ScheduledRoundClaim,
) -> WorkerResult {
    match diagnostics::catch_item_panic(
        "lifecycle",
        process_claim_inner(Arc::clone(&schedule), lifecycle, Arc::clone(&clock), claim.clone()),
    )
    .await
    {
        Ok(result) => result,
        Err(()) => {
            if tokio::time::timeout(
                std::time::Duration::from_secs(2),
                record_failure(
                    schedule,
                    clock,
                    claim,
                    "task_panic_terminal",
                    "工作项 panic，停止自动执行，需人工检查后恢复".to_owned(),
                ),
            )
            .await
            .is_err()
            {
                diagnostics::unpersisted(SafeErrorKind::TASK_PANIC);
            }
            WorkerResult::Error
        }
    }
}

#[tracing::instrument(

    skip_all,

    fields(

        round_id = %claim.round_id,

        auction_id = %claim.auction_id,

    )

)]
async fn process_claim_inner<C>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    claim: ScheduledRoundClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let action = claim.phase.action();
    let request = ScheduledLifecycleRequest { round_id: claim.round_id, phase: claim.phase };
    let request_hash = match RequestHash::from_request(
        action.command_kind(),
        claim.auction_id.as_uuid(),
        &request,
    ) {
        Ok(hash) => hash,
        Err(error) => {
            record_failure(
                schedule,
                clock,
                claim,
                "request_hash_failed",
                format!("无法生成 Worker 命令摘要: {error}"),
            )
            .await;
            return WorkerResult::Error;
        }
    };
    let command = AuctionLifecycleCommand {
        command_id: claim.command_id,
        auction_id: claim.auction_id,
        action,
        expected_state_version: None,
        reason: None,
        request_hash,
        request_id: None,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Worker,
            actor_id: claim.worker_id.clone(),
        },
    };
    match lifecycle.execute(command).await {
        Ok(_) => {
            if let Err(error) = schedule.complete(claim, clock.now()).await {
                diagnostics::error(
                    "auction_lifecycle",
                    "complete_claim",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                return WorkerResult::Error;
            }
            WorkerResult::Success
        }
        Err(error) => {
            record_failure(schedule, clock, claim, "lifecycle_failed", error.to_string()).await;
            WorkerResult::Error
        }
    }
}

pub(super) async fn record_failure<C>(
    schedule: Arc<dyn ScheduledRoundStore>,
    clock: Arc<C>,
    claim: ScheduledRoundClaim,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let now = clock.now();
    let Some(retry_at) = now.checked_add_signed(retry_delay(claim.attempt_count)) else {
        // 不把不可表示的退避时间改写为 now；claim 会在原有 fence 到期后重新变为可领取。
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!(failure_code, "lifecycle retry timestamp overflow; claim remains leased");
        return;
    };
    if let Err(error) = schedule.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await {
        diagnostics::error(
            "auction_lifecycle",
            "record_failure",
            error.diagnostic_kind(),
            DiagnosticContext {
                disposition: Some(DiagnosticDisposition::Unpersisted),
                ..Default::default()
            },
        );
    }
}

#[derive(Debug, Serialize)]
struct ScheduledLifecycleRequest {
    round_id: uuid::Uuid,
    phase: ScheduledRoundPhase,
}
