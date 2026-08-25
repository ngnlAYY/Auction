//! 结算履约与结算命令 claim 的处理。
//!
//! 准备、激活、证据对账和结算完成分别通过应用端口执行，Worker 只负责租约内的重试与结果
//! 记录，SQLite Lite 未注入的完整档端口不会被伪装为可用。

use super::*;

pub(super) async fn process_fulfillment_claim<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort>,
    clock: Arc<C>,
    metrics: Arc<dyn WorkerRecorder>,
    retry_delay: ChronoDuration,
    claim: SettlementFulfillmentClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let retry_claim = claim.clone();
    let mut worker_result = WorkerResult::Success;
    match claim.state {
        auction_application::SettlementFulfillmentState::Pending => {
            match fulfillment_commands.prepare(claim, clock.now()).await {
                Ok(result) => {
                    tracing::info!(
                        obligation_id = %result.obligation_id,
                        delivery_id = %result.delivery_id,
                        prepare_command_id = %result.prepare_command_id,
                        "settlement fulfillment prepared delivery outbox"
                    );
                }
                Err(error) => {
                    worker_result = WorkerResult::Error;
                    metrics
                        .observe_retry(WorkerLoop::AuctionFulfillment, WorkerRetryKind::Persisted);
                    record_fulfillment_failure(
                        fulfillment_store,
                        clock,
                        retry_claim,
                        retry_delay,
                        "delivery_unavailable",
                        error.to_string(),
                    )
                    .await;
                }
            }
        }
        auction_application::SettlementFulfillmentState::Prepared => {
            match fulfillment_commands.activate(claim, clock.now()).await {
                Ok(result) => {
                    tracing::info!(
                        obligation_id = %result.obligation_id,
                        delivery_id = %result.delivery_id,
                        activate_command_id = %result.activate_command_id,
                        "settlement fulfillment activated delivery outbox"
                    );
                }
                Err(error) => {
                    worker_result = WorkerResult::Error;
                    metrics
                        .observe_retry(WorkerLoop::AuctionFulfillment, WorkerRetryKind::Persisted);
                    record_fulfillment_failure(
                        fulfillment_store,
                        clock,
                        retry_claim,
                        retry_delay,
                        "delivery_unavailable",
                        error.to_string(),
                    )
                    .await;
                }
            }
        }
        auction_application::SettlementFulfillmentState::Enqueued => {
            match fulfillment_commands.reconcile(claim).await {
                Ok(result) => match result.status {
                    auction_application::SettlementFulfillmentReconcileStatus::Satisfied => {
                        tracing::info!(
                            obligation_id = %result.obligation_id,
                            delivery_id = %result.delivery_id,
                            "settlement fulfillment reconciled external Sent/Balance evidence"
                        );
                    }
                    auction_application::SettlementFulfillmentReconcileStatus::ManualReconcile => {
                        tracing::warn!(
                            obligation_id = %result.obligation_id,
                            delivery_id = %result.delivery_id,
                            "settlement fulfillment moved to manual reconciliation"
                        );
                    }
                },
                Err(error) => {
                    worker_result = WorkerResult::Error;
                    metrics
                        .observe_retry(WorkerLoop::AuctionFulfillment, WorkerRetryKind::Persisted);
                    record_fulfillment_failure(
                        fulfillment_store,
                        clock,
                        retry_claim,
                        retry_delay,
                        "evidence_unavailable",
                        error.to_string(),
                    )
                    .await;
                }
            }
        }
    }
    worker_result
}
pub(super) async fn record_fulfillment_failure<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    clock: Arc<C>,
    claim: SettlementFulfillmentClaim,
    retry_delay: ChronoDuration,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let now = clock.now();
    let Some(retry_at) = now.checked_add_signed(retry_delay) else {
        // 保留 claim 直到租约 fence 到期，避免用当前时间伪造一个可立即重试的退避点。
        tracing::error!(failure_code, "fulfillment retry timestamp overflow; claim remains leased");
        return;
    };
    if let Err(error) =
        fulfillment_store.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await
    {
        tracing::error!(error = %error, failure_code, "fulfillment worker failed to persist retry state");
    }
}

pub(super) async fn process_settlement_claim<C>(
    settlement_store: Arc<dyn SettlementWorkStore>,
    settlement_commands: Arc<dyn SettlementCommandPort>,
    clock: Arc<C>,
    claim: SettlementWorkClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if claim.completion_only {
        if let Err(error) = settlement_store.complete(claim, clock.now()).await {
            tracing::error!(error = %error, "settlement worker failed to persist completion fence");
            return WorkerResult::Error;
        }
        return WorkerResult::Success;
    }

    let request =
        SettlementWorkflowRequest { workflow_id: claim.workflow_id, due_at: claim.due_at };
    let request_hash =
        match RequestHash::from_request("settle_auction", claim.auction_id.as_uuid(), &request) {
            Ok(hash) => hash,
            Err(error) => {
                record_settlement_failure(
                    settlement_store,
                    clock,
                    claim,
                    "request_hash_failed",
                    format!("无法生成结算命令摘要: {error}"),
                )
                .await;
                return WorkerResult::Error;
            }
        };
    let command = SettleAuctionCommand {
        command_id: claim.settlement_command_id,
        auction_id: claim.auction_id,
        due_at: claim.due_at,
        now: clock.now(),
        request_hash,
    };
    match settlement_commands.settle(command).await {
        Ok(_) => {
            if let Err(error) = settlement_store.complete(claim, clock.now()).await {
                tracing::error!(error = %error, "settlement worker failed to persist completion");
                return WorkerResult::Error;
            }
            WorkerResult::Success
        }
        Err(error) => {
            record_settlement_failure(
                settlement_store,
                clock,
                claim,
                "settlement_failed",
                error.to_string(),
            )
            .await;
            WorkerResult::Error
        }
    }
}

pub(super) async fn record_settlement_failure<C>(
    settlement_store: Arc<dyn SettlementWorkStore>,
    clock: Arc<C>,
    claim: SettlementWorkClaim,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let now = clock.now();
    let Some(retry_at) = now.checked_add_signed(retry_delay(claim.attempt_count)) else {
        // 结算失败不能因为时间溢出而回到当前时刻；让 claim 按既有 fence 失效后再恢复。
        tracing::error!(failure_code, "settlement retry timestamp overflow; claim remains leased");
        return;
    };
    if let Err(error) =
        settlement_store.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await
    {
        tracing::error!(error = %error, failure_code, "settlement worker failed to persist retry state");
    }
}
#[derive(Debug, Serialize)]
struct SettlementWorkflowRequest {
    workflow_id: uuid::Uuid,
    due_at: DateTime<Utc>,
}
