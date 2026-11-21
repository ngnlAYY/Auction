//! 结算与履约持久任务的 Worker 编排；执行结果必须持久化，不以空端口冒充成功。

use std::sync::Arc;

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{ApplicationError, Clock};
use auction_application::settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentPreparationOutcome, SettlementFulfillmentReconcileStatus,
    SettlementFulfillmentState, SettlementFulfillmentStore, SettlementTransactionError,
    SettlementWorkClaim, SettlementWorkStore,
};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use auction_time::{TimeDelta, Timestamp};

use crate::worker::diagnostics;

const MAXIMUM_FULFILLMENT_PHASE_ADVANCES: usize = 4;

#[tracing::instrument(
    skip_all,
    fields(
        obligation_id = %claim.obligation_id,
        delivery_id = ?claim.delivery_id,
    )
)]
pub(crate) async fn process_fulfillment_claim<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort>,
    clock: Arc<C>,
    metrics: Arc<dyn WorkerRecorder>,
    retry_delay: TimeDelta,
    claim: SettlementFulfillmentClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let mut claim = claim;
    for _ in 0..MAXIMUM_FULFILLMENT_PHASE_ADVANCES {
        // 日志与重试指标只需标识和次数，不复制装备明细及 delivery 快照。
        let obligation_id = claim.obligation_id;
        let retry_count = claim.retry_count;
        if clock.now() >= claim.lease_until {
            diagnostics::unpersisted(SafeErrorKind::LEASE_EXPIRED);
            tracing::warn!(%obligation_id, "settlement fulfillment lease expired before phase");
            return WorkerResult::Error;
        }
        match claim.state {
            SettlementFulfillmentState::Pending => {
                let deadline = fulfillment_budget(&claim, clock.now());
                let Some(result) = guard_fulfillment_panic(
                    &fulfillment_store,
                    claim.clone(),
                    bounded_fulfillment(
                        deadline,
                        fulfillment_commands.prepare_next(claim, clock.now(), retry_delay),
                    ),
                )
                .await
                else {
                    return WorkerResult::Error;
                };
                match result {
                    Ok(SettlementFulfillmentPreparationOutcome::InternallySatisfied {
                        obligation_id,
                    }) => {
                        tracing::info!(
                            obligation_id = %obligation_id,
                            "settlement obligation internally satisfied"
                        );
                        return WorkerResult::Success;
                    }
                    Ok(SettlementFulfillmentPreparationOutcome::External(result)) => {
                        match *result {
                            SettlementFulfillmentPhaseResult::Advanced(result) => {
                                tracing::info!(
                                    obligation_id = %result.obligation_id,
                                    delivery_id = %result.delivery_id,
                                    prepare_command_id = %result.prepare_command_id,
                                    "settlement fulfillment prepared delivery outbox"
                                );
                                claim = result.next_claim;
                            }
                            SettlementFulfillmentPhaseResult::Deferred {
                                obligation_id,
                                failure_code,
                            } => {
                                observe_fulfillment_retry(metrics.as_ref(), retry_count);
                                tracing::info!(
                                    obligation_id = %obligation_id,
                                    failure_code,
                                    "settlement fulfillment dependency deferred before prepare"
                                );
                                return WorkerResult::Success;
                            }
                            SettlementFulfillmentPhaseResult::ManualReconcile {
                                obligation_id,
                                failure_code,
                            } => {
                                tracing::warn!(
                                    obligation_id = %obligation_id,
                                    failure_code,
                                    "settlement fulfillment stopped for manual reconciliation before prepare"
                                );
                                return WorkerResult::Success;
                            }
                        }
                    }
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "settlement_fulfillment",
                            "prepare_obligation",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                obligation_id: diagnostic_id(&obligation_id),
                                ..Default::default()
                            },
                        );

                        // 应用服务返回的错误尚未完成 fulfillment 状态迁移；旧版让 claim
                        // 保持原 lease/fence，交给下一轮重新领取，不能在这里猜测并覆盖
                        // 阶段专属 error_code。

                        return WorkerResult::Error;
                    }
                }
            }
            SettlementFulfillmentState::Prepared => {
                let now = clock.now();
                let deadline = fulfillment_budget(&claim, now);
                let Some(result) = guard_fulfillment_panic(
                    &fulfillment_store,
                    claim.clone(),
                    bounded_fulfillment(
                        deadline,
                        fulfillment_commands.activate(claim, now, retry_delay),
                    ),
                )
                .await
                else {
                    return WorkerResult::Error;
                };
                match result {
                    Ok(SettlementFulfillmentPhaseResult::Advanced(result)) => {
                        tracing::info!(
                            obligation_id = %result.obligation_id,
                            delivery_id = %result.delivery_id,
                            activate_command_id = %result.activate_command_id,
                            "settlement fulfillment activated delivery outbox"
                        );
                        claim = result.next_claim;
                    }
                    Ok(SettlementFulfillmentPhaseResult::Deferred {
                        obligation_id,
                        failure_code,
                    }) => {
                        observe_fulfillment_retry(metrics.as_ref(), retry_count);
                        tracing::info!(
                            obligation_id = %obligation_id,
                            failure_code,
                            "settlement fulfillment dependency deferred before activation"
                        );
                        return WorkerResult::Success;
                    }
                    Ok(SettlementFulfillmentPhaseResult::ManualReconcile {
                        obligation_id,
                        failure_code,
                    }) => {
                        tracing::warn!(
                            obligation_id = %obligation_id,
                            failure_code,
                            "settlement fulfillment stopped for manual reconciliation before activation"
                        );
                        return WorkerResult::Success;
                    }
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "settlement_fulfillment",
                            "activate_delivery",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                obligation_id: diagnostic_id(&obligation_id),
                                ..Default::default()
                            },
                        );

                        // 该错误没有对应的旧版持久化阶段结果；保留 claim 的 fencing，避免
                        // 把存储/状态错误误分类为 delivery_unavailable。

                        return WorkerResult::Error;
                    }
                }
            }
            SettlementFulfillmentState::Enqueued => {
                let observed_at = clock.now();
                let Some(retry_at) = observed_at.checked_add_signed(retry_delay) else {
                    diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
                    tracing::error!(
                        obligation_id = %claim.obligation_id,
                        "fulfillment reconcile retry timestamp overflow; claim remains leased"
                    );
                    return WorkerResult::Error;
                };
                let deadline = fulfillment_budget(&claim, observed_at);
                let delivery_id = claim.delivery_id;
                let Some(result) = guard_fulfillment_panic(
                    &fulfillment_store,
                    claim.clone(),
                    bounded_fulfillment(
                        deadline,
                        fulfillment_commands.reconcile(claim, observed_at, retry_at),
                    ),
                )
                .await
                else {
                    return WorkerResult::Error;
                };
                match result {
                    Ok(result) => {
                        match result.status {
                            SettlementFulfillmentReconcileStatus::Deferred => {
                                observe_fulfillment_retry(metrics.as_ref(), retry_count);
                                tracing::info!(
                                    obligation_id = %result.obligation_id,
                                    delivery_id = %result.delivery_id,
                                    "settlement fulfillment deferred until delivery/evidence advances"
                                );
                            }
                            SettlementFulfillmentReconcileStatus::Satisfied => {
                                tracing::info!(
                                    obligation_id = %result.obligation_id,
                                    delivery_id = %result.delivery_id,
                                    "settlement fulfillment reconciled external Sent/Balance evidence"
                                );
                            }
                            SettlementFulfillmentReconcileStatus::Failed => {
                                tracing::warn!(
                                    obligation_id = %result.obligation_id,
                                    delivery_id = %result.delivery_id,
                                    "settlement fulfillment moved to failed terminal state"
                                );
                            }
                            SettlementFulfillmentReconcileStatus::ManualReconcile => {
                                tracing::warn!(
                                    obligation_id = %result.obligation_id,
                                    delivery_id = %result.delivery_id,
                                    "settlement fulfillment moved to manual reconciliation"
                                );
                            }
                        }
                        return WorkerResult::Success;
                    }
                    Err(error) => {
                        // reconcile 的 dependency/证据错误若已被应用服务识别，会以 Deferred、
                        // Failed 或 ManualReconcile 返回；这里只处理未完成持久迁移的错误。
                        // 安全分类只补充诊断，保留失败关闭、原 claim 租约与重试，不记录错误正文。
                        diagnostics::unpersisted_error(
                            "settlement_fulfillment",
                            "reconcile_obligation",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                obligation_id: diagnostic_id(&obligation_id),
                                delivery_id,
                                ..Default::default()
                            },
                        );

                        return WorkerResult::Error;
                    }
                }
            }
        }
    }

    let obligation_id = claim.obligation_id;
    match fulfillment_store.manual_reconcile(claim, "invalid_fulfillment_fact".to_owned()).await {
        Ok(()) => {
            tracing::warn!(
                obligation_id = %obligation_id,
                "settlement fulfillment exceeded bounded phase advances and moved to manual reconciliation"
            );
            WorkerResult::Success
        }
        Err(error) => {
            diagnostics::error(
                "settlement_fulfillment",
                "persist_advance_limit",
                error.diagnostic_kind(),
                DiagnosticContext {
                    disposition: Some(DiagnosticDisposition::Unpersisted),
                    obligation_id: diagnostic_id(&obligation_id),
                    ..Default::default()
                },
            );
            WorkerResult::Error
        }
    }
}
/// 单一预算覆盖guard排队、全部远端读取、数据库等待与提交；超时直接销毁原future，绝不detach。
fn fulfillment_budget(claim: &SettlementFulfillmentClaim, now: Timestamp) -> std::time::Duration {
    (claim.lease_until - now)
        .to_std()
        .unwrap_or_default()
        .min(auction_application::settlement::INTERNAL_EVIDENCE_BUDGET)
}

async fn bounded_fulfillment<T>(
    budget: std::time::Duration,
    operation: impl std::future::Future<Output = Result<T, ApplicationError>>,
) -> Result<T, ApplicationError> {
    // timeout 会先 poll 内层 future；零预算不能启动远端证据读取。
    if budget.is_zero() {
        return Err(auction_application::core::ApplicationError::FulfillmentDependencyUnavailable);
    }
    tokio::time::timeout(budget, operation).await.map_err(|error| {
        diagnostics::warn(
            "settlement_fulfillment",
            "phase_timeout",
            diagnostics::timeout_kind(&error),
            DiagnosticContext::default(),
        );
        auction_application::core::ApplicationError::FulfillmentDependencyUnavailable
    })?
}

async fn guard_fulfillment_panic<T>(
    store: &Arc<dyn SettlementFulfillmentStore>,
    claim: SettlementFulfillmentClaim,
    future: impl Future<Output = Result<T, ApplicationError>>,
) -> Option<Result<T, ApplicationError>> {
    match diagnostics::catch_item_panic("settlement_fulfillment", future).await {
        Ok(result) => Some(result),
        Err(()) => {
            // 使用当前阶段的 fence；已提交的下一阶段不能被旧 claim 覆盖。
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                store.manual_reconcile(claim, "task_panic".to_owned()),
            )
            .await
            {
                Ok(Ok(())) => {}
                _ => diagnostics::unpersisted(SafeErrorKind::TASK_PANIC),
            }
            None
        }
    }
}

fn observe_fulfillment_retry(metrics: &dyn WorkerRecorder, retry_count: u32) {
    // 领取本身不增加 retry_count；只有此前至少释放过一次、本次 Deferred 再次释放时才计一次
    // 持久化重试，避免把首次延后算作重试。
    if retry_count > 0 {
        metrics.observe_retry(WorkerLoop::AuctionFulfillment, WorkerRetryKind::Persisted);
    }
}

// 结算命令幂等但 panic 原因未知；不把故障 owner 无限重新领取。
pub(crate) async fn process_settlement_claim<C: Clock + 'static>(
    settlement_store: Arc<dyn SettlementWorkStore>,
    settlement_commands: Arc<dyn SettlementCommandPort>,
    clock: Arc<C>,
    claim: SettlementWorkClaim,
) -> WorkerResult {
    match diagnostics::catch_item_panic(
        "settlement",
        process_settlement_claim_inner(
            Arc::clone(&settlement_store),
            settlement_commands,
            clock,
            claim.clone(),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(()) => {
            if tokio::time::timeout(
                std::time::Duration::from_secs(2),
                record_settlement_failure(
                    settlement_store,
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
        workflow_id = %claim.workflow_id,
        auction_id = %claim.auction_id,
    )
)]
async fn process_settlement_claim_inner<C>(
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
            diagnostics::error(
                "settlement",
                "complete_replayed_claim",
                error.diagnostic_kind(),
                DiagnosticContext {
                    disposition: Some(DiagnosticDisposition::Unpersisted),
                    ..Default::default()
                },
            );
            return WorkerResult::Error;
        }
        return WorkerResult::Success;
    }

    let request_hash = match auction_application::settlement::settlement_complete_request_hash(
        claim.auction_id.as_uuid(),
        claim.workflow_id,
        claim.due_at,
    ) {
        Ok(hash) => hash,
        Err(error) => {
            let safe_detail = match error.classify() {
                serde_json::error::Category::Io => "结算命令摘要序列化 I/O 失败",
                serde_json::error::Category::Syntax => "结算命令摘要格式无效",
                serde_json::error::Category::Data => "结算命令摘要数据无效",
                serde_json::error::Category::Eof => "结算命令摘要输出不完整",
            }
            .to_owned();
            record_settlement_failure(settlement_store, claim, "internal", safe_detail).await;
            return WorkerResult::Error;
        }
    };
    let command = SettleAuctionCommand {
        command_id: claim.settlement_command_id,
        workflow_id: claim.workflow_id,
        claim_command_id: claim.claim_command_id,
        auction_id: claim.auction_id,
        due_at: claim.due_at,
        now: claim.due_at,
        request_hash,
    };
    match settlement_commands.settle(command).await {
        Ok(_) => {
            if let Err(error) = settlement_store.complete(claim, clock.now()).await {
                diagnostics::error(
                    "settlement",
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
            let failure_code = settlement_failure_code(&error);
            record_settlement_failure(
                settlement_store,
                claim,
                failure_code,
                format!("结算处理失败，分类: {failure_code}"),
            )
            .await;
            WorkerResult::Error
        }
    }
}

fn settlement_failure_code(error: &auction_application::core::ApplicationError) -> &'static str {
    match error {
        auction_application::core::ApplicationError::Domain(_)
        | auction_application::core::ApplicationError::CommandLedger(
            auction_application::core::CommandLedgerError::Conflict,
        ) => "settlement_mismatch",
        auction_application::core::ApplicationError::Repository(_)
        | auction_application::core::ApplicationError::Serialization(_)
        | auction_application::core::ApplicationError::TransactionPolicy(_) => "persistence",
        auction_application::core::ApplicationError::SettlementTransaction(error) => match error {
            SettlementTransactionError::NotFound => "invalid_state",
            SettlementTransactionError::Conflict
            | SettlementTransactionError::CommandLedger(
                auction_application::core::CommandLedgerError::Conflict,
            ) => "settlement_mismatch",
            SettlementTransactionError::Failed(_)
            | SettlementTransactionError::SerializationConflict => "persistence",
            SettlementTransactionError::CommandLedger(_) => "internal",
        },
        _ => "internal",
    }
}

pub(crate) async fn record_settlement_failure(
    settlement_store: Arc<dyn SettlementWorkStore>,
    claim: SettlementWorkClaim,
    failure_code: &str,
    safe_detail: String,
) {
    // 退避间隔由存储按尝试次数决定；这里只提交失败分类与已脱敏说明。
    if let Err(error) = settlement_store.fail(claim, failure_code.to_owned(), safe_detail).await {
        diagnostics::error(
            "settlement",
            "record_failure",
            error.diagnostic_kind(),
            DiagnosticContext {
                disposition: Some(DiagnosticDisposition::Unpersisted),
                ..Default::default()
            },
        );
    }
}
