//! Worker 持久化运行时的装配与生命周期编排。
//!
//! 主循环只负责配置校验、任务启动、优雅排空和 Worker control 状态；具体业务处理按
//! delivery、recovery、生命周期和结算 owner 拆到相邻模块，避免在调度器中复制领域规则。

use std::sync::Arc;

use crate::WorkerControlHandle;
use auction_application::{
    ApplicationError, AuctionLifecycleActor, AuctionLifecycleActorType, AuctionLifecycleCommand,
    AuctionLifecycleCommandPort, Clock, DeliveryTransport, DeliveryTransportError,
    DeliveryWorkClaim, DeliveryWorkStore, MailDeliveryActivation, MailDeliveryCommand,
    MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
    RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_PROOF_KIND,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON, RecoveryActionOutcome, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklister, RecoveryResolutionStore, RecoveryWorkStore, RequestHash,
    ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore, SettleAuctionCommand,
    SettlementCommandPort, SettlementFulfillmentClaim, SettlementFulfillmentCommandPort,
    SettlementFulfillmentStore, SettlementWorkClaim, SettlementWorkStore,
};
use auction_domain::{CommandId, deterministic_uuid};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::Serialize;
use thiserror::Error;
use tokio::task::JoinSet;

#[path = "../auction/lifecycle.rs"]
mod auction;
#[path = "../forum/loop.rs"]
mod forum;
#[path = "../forum/reply.rs"]
mod forum_reply;
#[path = "../forum/sync.rs"]
mod forum_sync;
#[path = "../settlement/fulfillment.rs"]
mod fulfillment;
#[path = "../mail/poll.rs"]
mod mail_poll;
#[path = "../operations/alert.rs"]
mod operations_alert;
#[path = "../realtime/consumer.rs"]
mod realtime_consumer;
#[path = "../recovery/loop.rs"]
mod recovery;
#[path = "recovery.rs"]
mod recovery_actions;
#[path = "../recovery/health.rs"]
mod recovery_health;

pub use forum_reply::ForumReplyRuntime;
pub use forum_sync::ForumSyncRuntime;
pub use mail_poll::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeConfig,
};
pub use operations_alert::{OperationsAlertRuntime, OperationsAlertRuntimeConfig};
pub use realtime_consumer::{
    PersistentRealtimeConsumer, RealtimeConsumerConfigError, RealtimeConsumerRunner,
};
pub use recovery_health::RecoveryHealthRuntime;

mod common;
mod config;
mod delivery;
mod error;
mod lifecycle;
mod operations;
mod settlement;

pub use config::*;
pub use error::WorkerError;

use common::{
    bind_worker_metric_modes, drain_delivery_tasks, drain_worker_tasks, merge_worker_result,
    poll_delay, retry_delay, wait_until_or_shutdown,
};
use delivery::run_delivery_cycle;
use lifecycle::process_claim;
use operations::{OperationsAlertLoopAction, run_operations_alert_loop};
use recovery_actions::process_recovery_action;
use settlement::{process_fulfillment_claim, process_settlement_claim};

/// 无数据库时仅用于保持进程存活的启动骨架。
///
/// 真实 Worker 必须使用下面的 `run_persistent_until_shutdown`；该函数不执行任何任务，避免
/// 在没有持久化端口时误把内存等待器当成生产调度器。
pub async fn run_until_shutdown(
    concurrency: u16,
    shutdown: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    if concurrency == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "worker concurrency must be greater than zero",
        ));
    }
    tracing::info!(worker_concurrency = concurrency, "auction worker started");
    shutdown.await;
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
    pub auction: AuctionWorkerRuntimeConfig,
    pub settlement_store: Option<Arc<dyn SettlementWorkStore>>,
    pub settlement_commands: Option<Arc<dyn SettlementCommandPort>>,
    pub fulfillment_store: Option<Arc<dyn SettlementFulfillmentStore>>,
    pub fulfillment_commands: Option<Arc<dyn SettlementFulfillmentCommandPort>>,
    pub fulfillment: FulfillmentWorkerRuntimeConfig,
    pub recovery_health: Option<RecoveryHealthRuntime>,
    pub recovery_store: Option<Arc<dyn RecoveryWorkStore>>,
    pub recovery_resolution: Option<Arc<dyn RecoveryResolutionStore>>,
    pub recovery_delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub recovery_blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    pub recovery: RecoveryWorkerRuntimeConfig,
    pub delivery_store: Option<Arc<dyn DeliveryWorkStore>>,
    pub delivery_transport: Option<Arc<dyn DeliveryTransport>>,
    pub delivery: DeliveryWorkerRuntimeConfig,
    /// 每个启用的 MoogleMail world 都拥有独立的持久化游标和 gateway。
    pub mail_polls: Vec<mail_poll::MailPollRuntime>,
    pub forum: ForumWorkerRuntimeConfig,
    pub forum_sync: Option<forum_sync::ForumSyncRuntime>,
    pub forum_reply: Option<forum_reply::ForumReplyRuntime>,
    /// 可选 operations alert 扫描器；只在 PostgreSQL 完整档和明确启用时注入。
    pub operations_alert: Option<Arc<operations_alert::OperationsAlertRuntime>>,
    pub realtime_consumer: Option<Arc<dyn RealtimeConsumerRunner>>,
    /// Worker 固定 label 的指标端口；关闭指标时由装配根注入 no-op 实现。
    pub metrics: Arc<dyn WorkerRecorder>,
    /// Worker control 的进程状态和 MoogleMail 即时唤醒器。
    pub control: WorkerControlHandle,
    pub clock: Arc<C>,
    pub worker_id: String,
}

/// 使用持久化轮次表运行 Worker。
///
/// interval 只是唤醒信号；每一轮都会重新从数据库领取 due 且租约可用的记录。生命周期
/// 状态迁移统一调用 `auction-application`，本 crate 不复制领域状态机。
pub async fn run_persistent_until_shutdown<C, S>(
    schedule: Arc<dyn ScheduledRoundStore>,
    lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    clock: Arc<C>,
    worker_id: String,
    concurrency: u16,
    poll_interval: std::time::Duration,
    shutdown: S,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
    S: std::future::Future<Output = ()>,
{
    let auction_lease =
        std::time::Duration::from_secs(poll_interval.as_secs().saturating_mul(3).max(30));
    let auction_item_timeout = std::time::Duration::from_secs(30)
        .min(auction_lease.saturating_sub(std::time::Duration::from_millis(1)));
    run_persistent_with_settlement_until_shutdown(
        PersistentWorker {
            schedule,
            lifecycle,
            auction: AuctionWorkerRuntimeConfig {
                batch_limit: concurrency,
                concurrency,
                poll_interval,
                jitter: std::time::Duration::ZERO,
                lease_duration: auction_lease,
                item_timeout: auction_item_timeout,
            },
            settlement_store: None,
            settlement_commands: None,
            fulfillment_store: None,
            fulfillment_commands: None,
            fulfillment: FulfillmentWorkerRuntimeConfig::default(),
            recovery_health: None,
            recovery_store: None,
            recovery_resolution: None,
            recovery_delivery: None,
            recovery_blacklister: None,
            recovery: RecoveryWorkerRuntimeConfig::default(),
            delivery_store: None,
            delivery_transport: None,
            delivery: DeliveryWorkerRuntimeConfig::default(),
            mail_polls: Vec::new(),
            forum: ForumWorkerRuntimeConfig::default(),
            forum_sync: None,
            forum_reply: None,
            operations_alert: None,
            realtime_consumer: None,
            metrics: auction_observability::noop_worker_recorder(),
            control: WorkerControlHandle::new("unknown", 0),
            clock,
            worker_id,
        },
        shutdown,
    )
    .await
}

/// 使用持久化轮次表和结算 workflow 运行 Worker。
///
/// 结算参数只在 PostgreSQL 完整档注入；SQLite Lite 继续只运行已经声明支持的生命周期
/// 轮次。结算执行仍通过 `SettlementCommandPort` 进入应用层，Worker 不复制结算计算。
pub async fn run_persistent_with_settlement_until_shutdown<C, S>(
    worker: PersistentWorker<C>,
    shutdown: S,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
    S: std::future::Future<Output = ()>,
{
    if !worker.auction.validate() {
        return Err(WorkerError::InvalidAuctionConfiguration);
    }
    if !worker.delivery.validate() {
        return Err(WorkerError::InvalidDeliveryConfiguration);
    }
    if !worker.fulfillment.validate() {
        return Err(WorkerError::InvalidFulfillmentConfiguration);
    }
    if !worker.recovery.validate() {
        return Err(WorkerError::InvalidRecoveryConfiguration);
    }
    if !worker.forum.validate()
        || worker
            .forum_sync
            .as_ref()
            .is_some_and(|runtime| runtime.item_timeout >= worker.forum.lease_duration)
    {
        return Err(WorkerError::InvalidForumConfiguration);
    }
    if worker.fulfillment_store.is_some() != worker.fulfillment_commands.is_some() {
        return Err(WorkerError::InvalidFulfillmentConfiguration);
    }
    if worker.worker_id.is_empty() || worker.worker_id != worker.worker_id.trim() {
        return Err(WorkerError::InvalidWorkerIdentity);
    }

    let worker = Arc::new(worker);

    bind_worker_metric_modes(worker.metrics.as_ref());

    let shutdown = shutdown;
    tokio::pin!(shutdown);
    tracing::info!(
        worker_id = %worker.worker_id,
        worker_concurrency = worker.auction.concurrency,
        "persistent auction worker started"
    );
    worker.control.mark_serving();
    let (drain_sender, _) = tokio::sync::watch::channel(false);
    let poll_wakeup = worker.control.poll_wakeup();
    let mut poll_wakeup_wait = Box::pin(poll_wakeup.notified());
    let mut delivery_cycles = JoinSet::new();
    let delivery_enabled = worker.delivery_store.is_some() && worker.delivery_transport.is_some();
    if delivery_enabled {
        let delivery_worker = Arc::clone(&worker);
        delivery_cycles.spawn(async move { run_delivery_cycle(delivery_worker.as_ref()).await });
    }
    let mut delivery_scheduled_wake = tokio::time::Instant::now();
    let mut delivery_sleep = Box::pin(tokio::time::sleep(std::time::Duration::ZERO));
    let mut operations_alert_tasks = JoinSet::new();
    if let Some(runtime) = &worker.operations_alert {
        for action in [
            OperationsAlertLoopAction::Scan,
            OperationsAlertLoopAction::Dispatch,
            OperationsAlertLoopAction::Reconcile,
        ] {
            operations_alert_tasks.spawn(run_operations_alert_loop(
                Arc::clone(runtime),
                Arc::clone(&worker.clock),
                Arc::clone(&worker.metrics),
                action,
                drain_sender.subscribe(),
            ));
        }
    }
    let mut auction_tasks = JoinSet::new();
    auction_tasks.spawn(auction::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    let mut fulfillment_tasks = JoinSet::new();
    if worker.fulfillment_store.is_some() && worker.fulfillment_commands.is_some() {
        fulfillment_tasks
            .spawn(fulfillment::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut recovery_tasks = JoinSet::new();
    if worker.recovery_health.is_some()
        || worker.recovery_store.is_some()
        || worker.recovery_resolution.is_some()
    {
        recovery_tasks.spawn(recovery::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut mail_poll_tasks = JoinSet::new();
    for runtime in worker.mail_polls.iter().cloned() {
        mail_poll_tasks.spawn(mail_poll::run_loop(
            Arc::clone(&worker),
            runtime,
            drain_sender.subscribe(),
        ));
    }
    let mut forum_tasks = JoinSet::new();
    if worker.forum_sync.is_some() || worker.forum_reply.is_some() {
        forum_tasks.spawn(forum::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }
    let mut realtime_tasks = JoinSet::new();
    if worker.realtime_consumer.is_some() {
        realtime_tasks
            .spawn(realtime_consumer::run_loop(Arc::clone(&worker), drain_sender.subscribe()));
    }

    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => {
                worker.control.mark_draining();
                let _ = drain_sender.send(true);
                drain_delivery_tasks(&mut delivery_cycles).await?;
                drain_worker_tasks(&mut operations_alert_tasks, "operations alert").await?;
                drain_worker_tasks(&mut auction_tasks, "auction").await?;
                drain_worker_tasks(&mut fulfillment_tasks, "fulfillment").await?;
                drain_worker_tasks(&mut recovery_tasks, "recovery").await?;
                drain_worker_tasks(&mut mail_poll_tasks, "MoogleMail").await?;
                drain_worker_tasks(&mut forum_tasks, "forum").await?;
                drain_worker_tasks(&mut realtime_tasks, "realtime").await?;
                tracing::info!(worker_id = %worker.worker_id, "auction worker stopping");
                return Ok(());
            }
            _ = &mut delivery_sleep, if delivery_enabled && delivery_cycles.is_empty() =>
            {
                worker.metrics.observe_schedule_delay(
                    WorkerLoop::Delivery,
                    tokio::time::Instant::now().saturating_duration_since(delivery_scheduled_wake),
                );
                let delivery_worker = Arc::clone(&worker);
                delivery_cycles.spawn(async move {
                    run_delivery_cycle(delivery_worker.as_ref()).await
                });
            }
            result = delivery_cycles.join_next(), if !delivery_cycles.is_empty() => {
                match result {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(error))) => {
                        tracing::warn!(error = %error, "delivery worker cycle failed; retrying after poll delay");
                    }
                    Some(Err(error)) => {
                        worker.control.mark_degraded();
                        return Err(WorkerError::DeliveryTaskJoin(error));
                    }
                    None => {}
                }
                let wait = poll_delay(worker.delivery.poll_interval, worker.delivery.jitter);
                delivery_scheduled_wake = tokio::time::Instant::now() + wait;
                delivery_sleep.as_mut().reset(delivery_scheduled_wake);
            }
            result = operations_alert_tasks.join_next(), if !operations_alert_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::OperationsAlertTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::OperationsAlertTaskExited),
                }
            }
            result = auction_tasks.join_next(), if !auction_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::AuctionTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::AuctionTaskExited),
                }
            }
            result = fulfillment_tasks.join_next(), if !fulfillment_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::FulfillmentTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::FulfillmentTaskExited),
                }
            }
            result = recovery_tasks.join_next(), if !recovery_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::RecoveryTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::RecoveryTaskExited),
                }
            }
            result = mail_poll_tasks.join_next(), if !mail_poll_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::MailPollTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::MailPollTaskExited),
                }
            }
            result = forum_tasks.join_next(), if !forum_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::ForumTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::ForumTaskExited),
                }
            }
            result = realtime_tasks.join_next(), if !realtime_tasks.is_empty() => {
                worker.control.mark_degraded();
                match result {
                    Some(Err(error)) => return Err(WorkerError::RealtimeTaskJoin(error)),
                    Some(Ok(())) | None => return Err(WorkerError::RealtimeTaskExited),
                }
            }
            _ = &mut poll_wakeup_wait => {
                if worker.control.take_mail_poll_request() {
                    for runtime in &worker.mail_polls {
                        runtime.trigger();
                    }
                }
                poll_wakeup_wait = Box::pin(poll_wakeup.notified());
            }
        }
    }
}
