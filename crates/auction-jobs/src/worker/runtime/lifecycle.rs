//! 拍卖生命周期与结算轮次 claim 的应用命令执行。
//!
//! Worker 只生成稳定命令摘要并记录 claim 的完成/失败状态，实际状态迁移仍由应用端口拥有。

use super::*;

pub(super) async fn process_claim<C>(
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
                tracing::error!(error = %error, "worker failed to persist completed schedule");
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
        tracing::error!(failure_code, "lifecycle retry timestamp overflow; claim remains leased");
        return;
    };
    if let Err(error) = schedule.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await {
        tracing::error!(error = %error, failure_code, "worker failed to persist retry state");
    }
}

#[derive(Debug, Serialize)]
struct ScheduledLifecycleRequest {
    round_id: uuid::Uuid,
    phase: ScheduledRoundPhase,
}
