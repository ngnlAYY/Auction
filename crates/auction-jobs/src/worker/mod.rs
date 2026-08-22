use std::sync::Arc;

use auction_application::{
    ApplicationError, AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, Clock, DeliveryTransport, DeliveryWorkClaim, DeliveryWorkStore,
    RequestHash, ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore,
    SettleAuctionCommand, SettlementCommandPort, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentStore, SettlementWorkClaim,
    SettlementWorkStore,
};
use chrono::Duration as ChronoDuration;
use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

mod realtime_consumer;

pub use realtime_consumer::{
    PersistentRealtimeConsumer, RealtimeConsumerConfigError, RealtimeConsumerRunner,
};

/// Worker 调度器错误；数据库或应用错误只通过安全的应用错误分类向上返回。
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker concurrency must be greater than zero")]
    InvalidConcurrency,
    #[error("worker poll interval must be greater than zero")]
    InvalidPollInterval,
    #[error("worker identity must not be empty")]
    InvalidWorkerIdentity,
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error("worker shutdown signal failed")]
    ShutdownSignal(#[source] std::io::Error),
}

/// 无数据库时仅用于保持进程存活的启动骨架。
///
/// 真实 Worker 必须使用下面的 `run_persistent_until_shutdown`；该函数不执行任何任务，避免
/// 在没有持久化端口时误把内存等待器当成生产调度器。
pub async fn run_until_shutdown(concurrency: u16) -> std::io::Result<()> {
    if concurrency == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worker concurrency must be greater than zero",
        ));
    }
    tracing::info!(worker_concurrency = concurrency, "auction worker started");
    tokio::signal::ctrl_c().await?;
    tracing::info!("auction worker stopping");
    Ok(())
}

/// 持久化 Worker 的端口与运行参数。
///
/// PostgreSQL 完整档可以同时提供结算与履约端口；SQLite Lite 将这些完整档端口保持为 `None`，
/// 由装配根显式表达能力差异。
pub struct PersistentWorker<C> {
    pub schedule: Arc<dyn ScheduledRoundStore>,
    pub lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    pub settlement_store: Option<Arc<dyn SettlementWorkStore>>,
    pub settlement_commands: Option<Arc<dyn SettlementCommandPort>>,
    pub fulfillment_store: Option<Arc<dyn SettlementFulfillmentStore>>,
    pub fulfillment_commands: Option<Arc<dyn SettlementFulfillmentCommandPort>>,
    pub delivery_store: Option<Arc<dyn DeliveryWorkStore>>,
    pub delivery_transport: Option<Arc<dyn DeliveryTransport>>,
    pub realtime_consumer: Option<Arc<dyn RealtimeConsumerRunner>>,
    pub clock: Arc<C>,
    pub worker_id: String,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
}

/// 使用持久化轮次表运行 Worker。
///
/// interval 只是唤醒信号；每一轮都会重新从数据库领取 due 且租约可用的记录。生命周期
/// 状态迁移统一调用 `auction-application`，本 crate 不复制领域状态机。
pub async fn run_persistent_until_shutdown<C>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    worker_id: String,
    concurrency: u16,
    poll_interval: std::time::Duration,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    run_persistent_with_settlement_until_shutdown(PersistentWorker {
        schedule,
        lifecycle,
        settlement_store: None,
        settlement_commands: None,
        fulfillment_store: None,
        fulfillment_commands: None,
        delivery_store: None,
        delivery_transport: None,
        realtime_consumer: None,
        clock,
        worker_id,
        concurrency,
        poll_interval,
    })
    .await
}

/// 使用持久化轮次表和结算 workflow 运行 Worker。
///
/// 结算参数只在 PostgreSQL 完整档注入；SQLite Lite 继续只运行已经声明支持的生命周期
/// 轮次。结算执行仍通过 `SettlementCommandPort` 进入应用层，Worker 不复制结算计算。
pub async fn run_persistent_with_settlement_until_shutdown<C>(
    worker: PersistentWorker<C>,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    if worker.concurrency == 0 {
        return Err(WorkerError::InvalidConcurrency);
    }
    if worker.poll_interval.is_zero() {
        return Err(WorkerError::InvalidPollInterval);
    }
    if worker.worker_id.is_empty() || worker.worker_id != worker.worker_id.trim() {
        return Err(WorkerError::InvalidWorkerIdentity);
    }

    let mut interval = tokio::time::interval(worker.poll_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    tracing::info!(
        worker_id = %worker.worker_id,
        worker_concurrency = worker.concurrency,
        "persistent auction worker started"
    );

    loop {
        tokio::select! {
            result = &mut shutdown => {
                result.map_err(WorkerError::ShutdownSignal)?;
                tracing::info!(worker_id = %worker.worker_id, "auction worker stopping");
                return Ok(());
            }
            _ = interval.tick() => {
                run_cycle(&worker).await?;
            }
        }
    }
}

async fn run_cycle<C>(worker: &PersistentWorker<C>) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let now = worker.clock.now();
    let lease_until = now
        .checked_add_signed(lease_duration(worker.poll_interval))
        .ok_or(WorkerError::InvalidPollInterval)?;
    let claims = worker
        .schedule
        .claim_due(worker.worker_id.clone(), now, lease_until, worker.concurrency)
        .await?;
    if !claims.is_empty() {
        let mut tasks = JoinSet::new();
        for claim in claims {
            tasks.spawn(process_claim(
                Arc::clone(&worker.schedule),
                Arc::clone(&worker.lifecycle),
                Arc::clone(&worker.clock),
                claim,
            ));
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(error = %error, "worker task join failed");
            }
        }
    }

    if let (Some(settlement_store), Some(settlement_commands)) =
        (&worker.settlement_store, &worker.settlement_commands)
    {
        let settlement_claims = settlement_store
            .claim_due(worker.worker_id.clone(), now, lease_until, worker.concurrency)
            .await?;
        let mut tasks = JoinSet::new();
        for claim in settlement_claims {
            tasks.spawn(process_settlement_claim(
                Arc::clone(settlement_store),
                Arc::clone(settlement_commands),
                Arc::clone(&worker.clock),
                claim,
            ));
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(error = %error, "settlement worker task join failed");
            }
        }
    }

    if let (Some(fulfillment_store), Some(fulfillment_commands)) =
        (&worker.fulfillment_store, &worker.fulfillment_commands)
    {
        let fulfillment_claims = fulfillment_store
            .claim_due(worker.worker_id.clone(), now, lease_until, worker.concurrency)
            .await?;
        let mut tasks = JoinSet::new();
        for claim in fulfillment_claims {
            tasks.spawn(process_fulfillment_claim(
                Arc::clone(fulfillment_store),
                Arc::clone(fulfillment_commands),
                Arc::clone(&worker.clock),
                claim,
            ));
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(error = %error, "fulfillment worker task join failed");
            }
        }
    }

    if let Some(realtime_consumer) = &worker.realtime_consumer {
        match realtime_consumer.run_once(now).await {
            Ok(processed) if processed > 0 => {
                tracing::info!(processed, "realtime consumer advanced persisted cursor");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(error = %error, "realtime consumer cycle failed");
            }
        }
    }

    let (Some(delivery_store), Some(delivery_transport)) =
        (&worker.delivery_store, &worker.delivery_transport)
    else {
        return Ok(());
    };
    let delivery_claims = delivery_store
        .claim_due(worker.worker_id.clone(), now, lease_until, worker.concurrency)
        .await?;
    let mut tasks = JoinSet::new();
    for claim in delivery_claims {
        tasks.spawn(process_delivery_claim(
            Arc::clone(delivery_store),
            Arc::clone(delivery_transport),
            Arc::clone(&worker.clock),
            claim,
        ));
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!(error = %error, "delivery worker task join failed");
        }
    }
    Ok(())
}

async fn process_claim<C>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    claim: ScheduledRoundClaim,
) where
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
            return;
        }
    };
    let command = AuctionLifecycleCommand {
        command_id: claim.command_id,
        auction_id: claim.auction_id,
        action,
        request_hash,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Worker,
            actor_id: claim.worker_id.clone(),
        },
    };
    match lifecycle.execute(command).await {
        Ok(_) => {
            if let Err(error) = schedule.complete(claim, clock.now()).await {
                tracing::error!(error = %error, "worker failed to persist completed schedule");
            }
        }
        Err(error) => {
            record_failure(schedule, clock, claim, "lifecycle_failed", error.to_string()).await;
        }
    }
}

async fn record_failure<C>(
    schedule: Arc<dyn ScheduledRoundStore>,
    clock: Arc<C>,
    claim: ScheduledRoundClaim,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let retry_at = clock
        .now()
        .checked_add_signed(retry_delay(claim.attempt_count))
        .unwrap_or_else(|| clock.now());
    if let Err(error) = schedule.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await {
        tracing::error!(error = %error, failure_code, "worker failed to persist retry state");
    }
}

async fn process_fulfillment_claim<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    fulfillment_commands: Arc<dyn SettlementFulfillmentCommandPort>,
    clock: Arc<C>,
    claim: SettlementFulfillmentClaim,
) where
    C: Clock + 'static,
{
    let retry_claim = claim.clone();
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
                    record_fulfillment_failure(
                        fulfillment_store,
                        clock,
                        retry_claim,
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
                    record_fulfillment_failure(
                        fulfillment_store,
                        clock,
                        retry_claim,
                        "delivery_unavailable",
                        error.to_string(),
                    )
                    .await;
                }
            }
        }
    }
}

async fn process_delivery_claim<C>(
    delivery_store: Arc<dyn DeliveryWorkStore>,
    delivery_transport: Arc<dyn DeliveryTransport>,
    clock: Arc<C>,
    claim: DeliveryWorkClaim,
) where
    C: Clock + 'static,
{
    if let Err(error) = delivery_store.start_sending(claim.clone(), clock.now()).await {
        tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker failed to start sending");
        return;
    }
    match delivery_transport.send(claim.clone()).await {
        Ok(result) => {
            if let Err(error) = delivery_store.succeed(claim, result, clock.now()).await {
                tracing::error!(error = %error, "delivery worker failed to persist success");
            }
        }
        Err(error) => {
            if let Err(persist_error) = delivery_store.fail(claim, error, clock.now()).await {
                tracing::error!(error = %persist_error, "delivery worker failed to persist failure");
            }
        }
    }
}

async fn record_fulfillment_failure<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    clock: Arc<C>,
    claim: SettlementFulfillmentClaim,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let retry_at = clock
        .now()
        .checked_add_signed(retry_delay(claim.attempt_count))
        .unwrap_or_else(|| clock.now());
    if let Err(error) =
        fulfillment_store.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await
    {
        tracing::error!(error = %error, failure_code, "fulfillment worker failed to persist retry state");
    }
}

async fn process_settlement_claim<C>(
    settlement_store: Arc<dyn SettlementWorkStore>,
    settlement_commands: Arc<dyn SettlementCommandPort>,
    clock: Arc<C>,
    claim: SettlementWorkClaim,
) where
    C: Clock + 'static,
{
    if claim.completion_only {
        if let Err(error) = settlement_store.complete(claim, clock.now()).await {
            tracing::error!(error = %error, "settlement worker failed to persist completion fence");
        }
        return;
    }

    let request = SettlementWorkflowRequest { workflow_id: claim.workflow_id };
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
                return;
            }
        };
    let command = SettleAuctionCommand {
        command_id: claim.settlement_command_id,
        auction_id: claim.auction_id,
        request_hash,
    };
    match settlement_commands.settle(command).await {
        Ok(_) => {
            if let Err(error) = settlement_store.complete(claim, clock.now()).await {
                tracing::error!(error = %error, "settlement worker failed to persist completion");
            }
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
        }
    }
}

async fn record_settlement_failure<C>(
    settlement_store: Arc<dyn SettlementWorkStore>,
    clock: Arc<C>,
    claim: SettlementWorkClaim,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let retry_at = clock
        .now()
        .checked_add_signed(retry_delay(claim.attempt_count))
        .unwrap_or_else(|| clock.now());
    if let Err(error) =
        settlement_store.fail(claim, retry_at, failure_code.to_owned(), safe_detail).await
    {
        tracing::error!(error = %error, failure_code, "settlement worker failed to persist retry state");
    }
}

#[derive(Debug, Serialize)]
struct ScheduledLifecycleRequest {
    round_id: uuid::Uuid,
    phase: ScheduledRoundPhase,
}

#[derive(Debug, Serialize)]
struct SettlementWorkflowRequest {
    workflow_id: uuid::Uuid,
}

fn lease_duration(poll_interval: std::time::Duration) -> ChronoDuration {
    let seconds = poll_interval.as_secs().saturating_mul(3).max(30);
    ChronoDuration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
}

fn retry_delay(attempt_count: u32) -> ChronoDuration {
    let exponent = attempt_count.saturating_sub(1).min(8);
    let seconds = 5_u64.saturating_mul(1_u64 << exponent).min(15 * 60);
    ChronoDuration::seconds(i64::try_from(seconds).unwrap_or(15 * 60))
}
