use std::sync::Arc;

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

#[path = "auction/lifecycle.rs"]
mod auction;
#[path = "control/mod.rs"]
mod control;
#[path = "forum/loop.rs"]
mod forum;
#[path = "forum/reply.rs"]
mod forum_reply;
#[path = "forum/sync.rs"]
mod forum_sync;
#[path = "settlement/fulfillment.rs"]
mod fulfillment;
#[path = "mail/poll.rs"]
mod mail_poll;
#[path = "operations/alert.rs"]
mod operations_alert;
#[path = "realtime/consumer.rs"]
mod realtime_consumer;
#[path = "recovery/loop.rs"]
mod recovery;
#[path = "recovery/health.rs"]
mod recovery_health;

pub use control::{
    LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult,
    WORKER_CONTROL_LIVE_PATH, WORKER_CONTROL_METRICS_PATH, WORKER_CONTROL_PROTOCOL_VERSION,
    WORKER_CONTROL_READY_PATH, WORKER_CONTROL_STATUS_PATH, WORKER_CONTROL_TRIGGER_POLL_PATH,
    WorkerControlClient, WorkerControlClientError, WorkerControlHandle, WorkerControlServer,
    WorkerControlServerConfig, WorkerControlServerError,
};
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

/// Worker 调度器错误；数据库或应用错误只通过安全的应用错误分类向上返回。
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker concurrency must be greater than zero")]
    InvalidConcurrency,
    #[error("worker poll interval must be greater than zero")]
    InvalidPollInterval,
    #[error("worker auction loop configuration is invalid")]
    InvalidAuctionConfiguration,
    #[error("auction worker task failed")]
    AuctionTaskJoin(#[source] tokio::task::JoinError),
    #[error("auction worker task exited unexpectedly")]
    AuctionTaskExited,
    #[error("worker recovery retry delay must be between 1 and 86400 seconds")]
    InvalidRecoveryRetryDelay,
    #[error("worker recovery batch limit must be between 1 and 1000")]
    InvalidRecoveryBatchLimit,
    #[error("worker recovery concurrency must be between 1 and recovery batch limit")]
    InvalidRecoveryConcurrency,
    #[error("worker delivery loop configuration is invalid")]
    InvalidDeliveryConfiguration,
    #[error("worker fulfillment loop configuration is invalid")]
    InvalidFulfillmentConfiguration,
    #[error("fulfillment worker task failed")]
    FulfillmentTaskJoin(#[source] tokio::task::JoinError),
    #[error("fulfillment worker task exited unexpectedly")]
    FulfillmentTaskExited,
    #[error("worker recovery loop configuration is invalid")]
    InvalidRecoveryConfiguration,
    #[error("recovery worker task failed")]
    RecoveryTaskJoin(#[source] tokio::task::JoinError),
    #[error("recovery worker task exited unexpectedly")]
    RecoveryTaskExited,
    #[error("delivery worker cycle task failed")]
    DeliveryTaskJoin(#[source] tokio::task::JoinError),
    #[error("operations alert worker task failed")]
    OperationsAlertTaskJoin(#[source] tokio::task::JoinError),
    #[error("operations alert worker task exited unexpectedly")]
    OperationsAlertTaskExited,
    #[error("worker recovery health refresh interval must be greater than zero")]
    InvalidRecoveryHealthRefreshInterval,
    #[error("worker identity must not be empty")]
    InvalidWorkerIdentity,
    #[error("MoogleMail poll configuration is invalid")]
    InvalidMailPollConfiguration,
    #[error("MoogleMail poll worker task failed")]
    MailPollTaskJoin(#[source] tokio::task::JoinError),
    #[error("MoogleMail poll worker task exited unexpectedly")]
    MailPollTaskExited,
    #[error("forum sync configuration is invalid")]
    InvalidForumSyncConfiguration,
    #[error("forum reply configuration is invalid")]
    InvalidForumReplyConfiguration,
    #[error("forum worker loop configuration is invalid")]
    InvalidForumConfiguration,
    #[error("forum worker task failed")]
    ForumTaskJoin(#[source] tokio::task::JoinError),
    #[error("forum worker task exited unexpectedly")]
    ForumTaskExited,
    #[error("realtime consumer worker task failed")]
    RealtimeTaskJoin(#[source] tokio::task::JoinError),
    #[error("realtime consumer worker task exited unexpectedly")]
    RealtimeTaskExited,
    #[error("{group} worker task failed while draining")]
    DrainTaskJoin {
        group: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error(transparent)]
    Application(#[from] ApplicationError),
}

/// Delivery loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct DeliveryWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub renew_interval: std::time::Duration,
    pub sending_grace: std::time::Duration,
    pub request_timeout: std::time::Duration,
    pub finalize_timeout: std::time::Duration,
}

impl Default for DeliveryWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            renew_interval: std::time::Duration::from_secs(10),
            sending_grace: std::time::Duration::from_secs(120),
            request_timeout: std::time::Duration::from_secs(30),
            finalize_timeout: std::time::Duration::from_secs(5),
        }
    }
}

impl DeliveryWorkerRuntimeConfig {
    fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && !self.renew_interval.is_zero()
            && self.renew_interval < self.lease_duration
            && !self.request_timeout.is_zero()
            && self.sending_grace > self.request_timeout
            && !self.finalize_timeout.is_zero()
    }
}

/// Auction 生命周期与结算 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct AuctionWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
}

impl Default for AuctionWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(180),
            item_timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl AuctionWorkerRuntimeConfig {
    fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
    }
}

/// Forum 主题发布与回帖导入 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct ForumWorkerRuntimeConfig {
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
}

impl Default for ForumWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(300),
        }
    }
}

impl ForumWorkerRuntimeConfig {
    fn validate(self) -> bool {
        !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
    }
}

/// Fulfillment loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct FulfillmentWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
    pub retry_delay: std::time::Duration,
}

impl Default for FulfillmentWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            item_timeout: std::time::Duration::from_secs(30),
            retry_delay: std::time::Duration::from_secs(60),
        }
    }
}

impl FulfillmentWorkerRuntimeConfig {
    fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
            && !self.retry_delay.is_zero()
            && self.retry_delay <= std::time::Duration::from_secs(86_400)
    }
}

/// Recovery loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct RecoveryWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
    pub retry_delay: std::time::Duration,
}

impl Default for RecoveryWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            item_timeout: std::time::Duration::from_secs(30),
            retry_delay: std::time::Duration::from_secs(60),
        }
    }
}

impl RecoveryWorkerRuntimeConfig {
    fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
            && !self.retry_delay.is_zero()
            && self.retry_delay <= std::time::Duration::from_secs(86_400)
    }
}

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

/// 在固定 poll 后附加旧版同语义的 `[0, jitter]` 进程随机抖动，避免多实例同时抢占。
fn poll_delay(
    poll_interval: std::time::Duration,
    jitter: std::time::Duration,
) -> std::time::Duration {
    if jitter.is_zero() {
        return poll_interval;
    }
    let jitter_nanos = jitter.as_nanos();
    let random_sample = uuid::Uuid::new_v4().as_u128();
    let offset_nanos = random_sample % (jitter_nanos + 1);
    let offset_nanos = u64::try_from(offset_nanos).unwrap_or(u64::MAX);
    poll_interval.saturating_add(std::time::Duration::from_nanos(offset_nanos))
}

#[derive(Debug, Clone, Copy)]
enum OperationsAlertLoopAction {
    Scan,
    Dispatch,
    Reconcile,
}

impl OperationsAlertLoopAction {
    const fn loop_kind(self) -> WorkerLoop {
        match self {
            Self::Scan => WorkerLoop::OperationsAlertScan,
            Self::Dispatch => WorkerLoop::OperationsAlertDispatch,
            Self::Reconcile => WorkerLoop::OperationsAlertReconcile,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Dispatch => "dispatch",
            Self::Reconcile => "reconcile",
        }
    }

    const fn timeout(self, config: OperationsAlertRuntimeConfig) -> std::time::Duration {
        match self {
            Self::Scan => config.scan_timeout,
            Self::Dispatch => config.dispatch_timeout,
            Self::Reconcile => config.reconcile_timeout,
        }
    }
}

/// 独立运行一个 operations alert owner；单轮错误只影响自身下一次轮询。
async fn run_operations_alert_loop<C>(
    runtime: Arc<OperationsAlertRuntime>,
    clock: Arc<C>,
    metrics: Arc<dyn WorkerRecorder>,
    action: OperationsAlertLoopAction,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        let started = std::time::Instant::now();
        let now = clock.now();
        let attempt = tokio::time::timeout(action.timeout(runtime.config), async {
            match action {
                OperationsAlertLoopAction::Scan => runtime.scan(now).await.map(|()| None),
                OperationsAlertLoopAction::Dispatch => runtime.dispatch(now).await.map(|()| None),
                OperationsAlertLoopAction::Reconcile => {
                    runtime.reconcile(now, runtime.config.reconcile_limit).await.map(Some)
                }
            }
        })
        .await;
        let metric_result = match attempt {
            Ok(Ok(processed)) => {
                if let Some(processed) = processed {
                    metrics.observe_batch(
                        WorkerLoop::OperationsAlertReconcile,
                        processed,
                        usize::from(runtime.config.reconcile_limit),
                    );
                }
                WorkerResult::Success
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    error = %error,
                    operation = action.name(),
                    "operations alert worker cycle failed; retrying after poll delay"
                );
                WorkerResult::Error
            }
            Err(_) => {
                tracing::warn!(
                    operation = action.name(),
                    "operations alert worker cycle timed out; retrying after poll delay"
                );
                WorkerResult::Error
            }
        };
        metrics.observe_iteration(action.loop_kind(), metric_result, started.elapsed());

        let scheduled_wake = tokio::time::Instant::now() + runtime.config.poll_interval;
        if wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
        metrics.observe_schedule_delay(
            action.loop_kind(),
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}

async fn drain_delivery_tasks(
    tasks: &mut JoinSet<Result<(), WorkerError>>,
) -> Result<(), WorkerError> {
    while let Some(result) = tasks.join_next().await {
        result.map_err(WorkerError::DeliveryTaskJoin)??;
    }
    Ok(())
}

async fn drain_worker_tasks(
    tasks: &mut JoinSet<()>,
    group: &'static str,
) -> Result<(), WorkerError> {
    while let Some(result) = tasks.join_next().await {
        result.map_err(|source| WorkerError::DrainTaskJoin { group, source })?;
    }
    Ok(())
}

/// 等待下一轮 deadline；收到 drain 后立即跳过休眠，且不再领取新工作。
pub(super) async fn wait_until_or_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        biased;
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        () = tokio::time::sleep_until(deadline) => false,
    }
}

/// 以独立 cadence 和 bounded concurrency 处理 delivery outbox。
///
/// 只有获得空闲并发槽后才领取下一条记录，避免已经领取但尚未执行的任务在队列中耗尽 lease。
/// 凭据暂不可用时释放当前 claim 并停止本轮，防止同一条 pending 记录被紧密重复领取。
async fn run_delivery_cycle<C>(worker: &PersistentWorker<C>) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    let (Some(delivery_store), Some(delivery_transport)) =
        (&worker.delivery_store, &worker.delivery_transport)
    else {
        return Ok(());
    };
    let delivery_started = std::time::Instant::now();
    let delivery_lease = ChronoDuration::from_std(worker.delivery.lease_duration)
        .map_err(|_| WorkerError::InvalidDeliveryConfiguration)?;
    let mut delivery_count = 0usize;
    let mut delivery_result = WorkerResult::Success;
    let mut claim_error: Option<ApplicationError> = None;
    let mut tasks = JoinSet::new();

    while delivery_count < usize::from(worker.delivery.batch_limit) {
        if tasks.len() >= usize::from(worker.delivery.concurrency)
            && let Some(result) = tasks.join_next().await
        {
            merge_delivery_task_result(&mut delivery_result, result);
        }

        let now = worker.clock.now();
        let Some(lease_until) = now.checked_add_signed(delivery_lease) else {
            claim_error =
                Some(ApplicationError::Repository(auction_application::RepositoryError::Failed(
                    "delivery claim lease 截止时间溢出".to_owned(),
                )));
            break;
        };
        let mut claims = match delivery_store
            .claim_due(worker.worker_id.clone(), now, lease_until, worker.delivery.sending_grace, 1)
            .await
        {
            Ok(claims) => claims,
            Err(error) => {
                claim_error = Some(error);
                break;
            }
        };
        let Some(claim) = claims.pop() else {
            break;
        };
        delivery_count += 1;
        if claim.attempt_count > 1 {
            worker.metrics.observe_retry(WorkerLoop::Delivery, WorkerRetryKind::Persisted);
        }

        if !delivery_transport.available(&claim) {
            let release = tokio::time::timeout(
                worker.delivery.finalize_timeout,
                delivery_store.release(claim.clone(), worker.clock.now()),
            )
            .await;
            match release {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker failed to release unavailable claim");
                    delivery_result = WorkerResult::Error;
                }
                Err(_) => {
                    tracing::error!(delivery_id = %claim.delivery_id, "delivery worker timed out while releasing unavailable claim");
                    delivery_result = WorkerResult::Error;
                }
            }
            break;
        }

        tasks.spawn(process_delivery_claim(
            Arc::clone(delivery_store),
            Arc::clone(delivery_transport),
            Arc::clone(&worker.clock),
            worker.delivery,
            claim,
        ));
    }

    while let Some(result) = tasks.join_next().await {
        merge_delivery_task_result(&mut delivery_result, result);
    }
    if claim_error.is_none() && delivery_result == WorkerResult::Success {
        worker.metrics.observe_batch(
            WorkerLoop::Delivery,
            delivery_count,
            usize::from(worker.delivery.batch_limit),
        );
    }
    if claim_error.is_some() {
        delivery_result = WorkerResult::Error;
    }
    worker.metrics.observe_iteration(
        WorkerLoop::Delivery,
        delivery_result,
        delivery_started.elapsed(),
    );
    match claim_error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn merge_delivery_task_result(
    delivery_result: &mut WorkerResult,
    result: Result<WorkerResult, tokio::task::JoinError>,
) {
    match result {
        Ok(WorkerResult::Success) => {}
        Ok(WorkerResult::Error) => *delivery_result = WorkerResult::Error,
        Ok(WorkerResult::Canceled) => *delivery_result = WorkerResult::Canceled,
        Err(error) => {
            tracing::error!(error = %error, "delivery worker task join failed");
            *delivery_result = WorkerResult::Canceled;
        }
    }
}

async fn process_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if !matches!(
        claim.kind.as_str(),
        "payment_reminder"
            | "reclaim_equipment_take"
            | "cancel_carrier_take"
            | "recover_equipment_take"
            | "buyer_blacklist"
    ) {
        let failure_code = "unsupported_recovery_action";
        return match store.mark_action_manual_reconcile(claim, failure_code.to_owned()).await {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery action unsupported-kind fence failed");
                WorkerResult::Error
            }
        };
    }

    if claim.kind == "buyer_blacklist" {
        let Some(blacklister) = blacklister else {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_buyer_blacklist_owner_unavailable",
            )
            .await;
        };
        return process_recovery_buyer_blacklist(store, blacklister, clock, retry_delay, claim)
            .await;
    }

    let Some(delivery) = delivery else {
        return release_recovery_action(
            store,
            clock,
            claim,
            retry_delay,
            "recovery_delivery_unavailable",
        )
        .await;
    };

    if claim.execution_state == "pending" {
        let recipient = match claim.recipient.clone() {
            Some(recipient) => recipient,
            None => {
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_recipient_missing".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(error) => {
                        tracing::error!(error = %error, "recovery action recipient fence failed");
                        WorkerResult::Error
                    }
                };
            }
        };
        if !claim.recipient_eligible {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_recipient_ineligible",
            )
            .await;
        }
        let action_id = claim.action_id.as_uuid();
        let is_payment_reminder = claim.kind == "payment_reminder";
        let external_mail_id = if is_payment_reminder {
            claim.payment_reference.clone()
        } else {
            let Some(source_external_mail_id) = claim.source_external_mail_id.clone() else {
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_source_mail_missing".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(error) => {
                        tracing::error!(error = %error, "recovery action source mail fence failed");
                        WorkerResult::Error
                    }
                };
            };
            source_external_mail_id
        };
        let command_id = CommandId::from_uuid(deterministic_uuid(
            CommandId::from_uuid(action_id),
            if is_payment_reminder {
                "recovery-payment-reminder-prepare"
            } else {
                "recovery-take-prepare"
            },
            1,
        ));
        let request_hash = if is_payment_reminder {
            let request = RecoveryPaymentReminderRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: recipient.clone(),
                payment_reference: claim.payment_reference.clone(),
            };
            RequestHash::from_request("recovery_payment_reminder", action_id, &request)
        } else {
            let request = RecoveryTakeRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: recipient.clone(),
                source_external_mail_id: external_mail_id.clone(),
                expected_credits: claim.expected_credits,
                expected_cod: claim.expected_cod,
                expected_attachment_ids: claim.expected_attachment_ids.clone(),
            };
            RequestHash::from_request("recovery_take", action_id, &request)
        };
        let request_hash = match request_hash {
            Ok(hash) => hash,
            Err(error) => {
                tracing::error!(error = %error, "recovery payment reminder request hash failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_request_hash_failed",
                )
                .await;
            }
        };
        let command = MailDeliveryCommand {
            command_id,
            request_hash,
            origin: MailDeliveryOrigin::SettlementRecoveryAction,
            origin_id: format!("recovery:{}", claim.recovery_id),
            operation: if is_payment_reminder {
                MailDeliveryOperation::Send
            } else {
                MailDeliveryOperation::Take
            },
            // 付款提醒固定发往 main；退回领取动作使用 recovery action 对应的目标世界。
            world_id: if is_payment_reminder { "main".to_owned() } else { claim.world_id.clone() },
            external_mail_id,
            recipient,
            template: is_payment_reminder
                .then_some(MailNotificationTemplate::RecoveryPaymentReminder),
            result_code: is_payment_reminder.then_some("payment_overdue".to_owned()),
            user_key_user_id: None,
            credits: if is_payment_reminder { 0 } else { claim.expected_credits },
            cod: if is_payment_reminder { 0 } else { claim.expected_cod },
            attachment_ids: if is_payment_reminder {
                Vec::new()
            } else {
                claim.expected_attachment_ids.clone()
            },
            notification_body: None,
        };
        let delivery_id = match delivery.prepare_recovery(command).await {
            Ok(result) if !result.delivery_id.is_nil() => result.delivery_id,
            Ok(_) => {
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_result_invalid",
                )
                .await;
            }
            Err(error) => {
                tracing::warn!(error = %error, "recovery delivery prepare failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_unavailable",
                )
                .await;
            }
        };
        let linked = match store
            .link_action_preparation(
                claim.clone(),
                delivery_id,
                command_id,
                request_hash,
                clock.now(),
            )
            .await
        {
            Ok(claim) => claim,
            Err(error) => {
                tracing::error!(error = %error, "recovery action prepared link failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_link_failed",
                )
                .await;
            }
        };
        return activate_recovery_action(store, delivery, clock, retry_delay, linked).await;
    }

    if claim.execution_state == "prepared" {
        if !claim.recipient_eligible {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_recipient_ineligible",
            )
            .await;
        }
        return activate_recovery_action(store, delivery, clock, retry_delay, claim).await;
    }

    if claim.execution_state == "enqueued" {
        return observe_recovery_action(store, clock, retry_delay, claim).await;
    }

    tracing::error!(state = %claim.execution_state, "recovery action state is not executable");
    WorkerResult::Error
}

/// 激活 recovery 的 prepared delivery，并以 action fence 记录 activation identity。
///
/// delivery 状态和 recovery action 不在同一张表中，因此允许在两次本地提交之间崩溃；
/// 下一轮会用相同 identity 重放 activation，再由 action fence 决定是否能够继续观察结果。
async fn activate_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Arc<dyn MailDeliveryEnqueuer>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return match store
            .mark_action_manual_reconcile(claim, "recovery_prepare_identity_missing".to_owned())
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery prepare identity fence failed");
                WorkerResult::Error
            }
        };
    };
    let activate_command_id = CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(claim.action_id.as_uuid()),
        "recovery-delivery-activate",
        1,
    ));
    let request = RecoveryActivationRequest {
        action_id: claim.action_id.as_uuid(),
        delivery_id,
        prepare_command_id: prepare_command_id.as_uuid(),
        prepare_request_hash: prepare_request_hash.as_bytes(),
    };
    let activate_request_hash = match RequestHash::from_request(
        "recovery_delivery_activation",
        claim.action_id.as_uuid(),
        &request,
    ) {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(error = %error, "recovery delivery activation hash failed");
            return match store
                .mark_action_manual_reconcile(claim, "recovery_activation_hash_failed".to_owned())
                .await
            {
                Ok(()) => WorkerResult::Success,
                Err(fence_error) => {
                    tracing::error!(error = %fence_error, "recovery activation hash fence failed");
                    WorkerResult::Error
                }
            };
        }
    };
    let activated_at = clock.now();
    let activation = MailDeliveryActivation {
        delivery_id,
        prepare_command_id,
        prepare_request_hash,
        activate_command_id,
        activate_request_hash,
        activated_at,
    };
    let result = match delivery.activate_recovery(activation).await {
        Ok(result) if result.delivery_id == delivery_id => result,
        Ok(_) => {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_result_invalid",
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(error = %error, "recovery delivery activation failed");
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_unavailable",
            )
            .await;
        }
    };
    let _ = result;
    let enqueued = match store
        .mark_action_enqueued(
            claim.clone(),
            activate_command_id,
            activate_request_hash,
            activated_at,
        )
        .await
    {
        Ok(claim) => claim,
        Err(error) => {
            tracing::warn!(error = %error, "recovery action activation link failed");
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_link_failed",
            )
            .await;
        }
    };
    observe_recovery_action(store, clock, retry_delay, enqueued).await
}

async fn process_recovery_buyer_blacklist<C>(
    store: Arc<dyn RecoveryWorkStore>,
    blacklister: Arc<dyn RecoveryBuyerBlacklister>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if claim.execution_state != "pending" {
        tracing::error!(
            state = %claim.execution_state,
            "recovery buyer blacklist action state is not executable"
        );
        return WorkerResult::Error;
    }
    let action_id = claim.action_id.as_uuid();
    let command_id = CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(action_id),
        "recovery-buyer-blacklist-command",
        1,
    ));
    let request = RecoveryBuyerBlacklistRequest {
        recovery_id: claim.recovery_id.as_uuid(),
        action_id,
        buyer_user_id: claim.buyer_user_id,
        public_reason: RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON,
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR,
        occurred_at: claim.created_at,
    };
    let request_hash =
        match RequestHash::from_request("users_recovery_buyer_blacklist", action_id, &request) {
            Ok(hash) => hash,
            Err(error) => {
                tracing::error!(error = %error, "recovery buyer blacklist request hash failed");
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_request_hash_failed".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(fence_error) => {
                        tracing::error!(
                            error = %fence_error,
                            "recovery buyer blacklist manual fence failed"
                        );
                        WorkerResult::Error
                    }
                };
            }
        };
    let command = RecoveryBuyerBlacklistCommand {
        command_id: command_id.as_uuid(),
        request_hash,
        recovery_id: claim.recovery_id,
        action_id: claim.action_id,
        buyer_user_id: claim.buyer_user_id,
        public_reason: RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON.to_owned(),
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR.to_owned(),
        occurred_at: claim.created_at,
    };
    match blacklister.ensure_buyer_blacklisted(command).await {
        Ok(result)
            if result.validate()
                && result.command_id == command_id.as_uuid()
                && result.recovery_id == claim.recovery_id
                && result.action_id == claim.action_id
                && result.buyer_user_id == claim.buyer_user_id
                && result.occurred_at == claim.created_at =>
        {
            match store
                .mark_action_satisfied(
                    claim,
                    RECOVERY_BUYER_BLACKLIST_PROOF_KIND.to_owned(),
                    result.command_id.to_string(),
                    clock.now(),
                )
                .await
            {
                Ok(()) => WorkerResult::Success,
                Err(error) => {
                    tracing::error!(error = %error, "recovery buyer blacklist proof fence failed");
                    WorkerResult::Error
                }
            }
        }
        Ok(_) => match store
            .mark_action_manual_reconcile(
                claim,
                "recovery_buyer_blacklist_result_invalid".to_owned(),
            )
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery buyer blacklist result fence failed");
                WorkerResult::Error
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "recovery buyer blacklist owner failed");
            release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_buyer_blacklist_unavailable",
            )
            .await
        }
    }
}

async fn observe_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    match store.observe_action(claim.clone(), clock.now()).await {
        Ok(RecoveryActionOutcome::Retry) => {
            release_recovery_action(store, clock, claim, retry_delay, "recovery_delivery_pending")
                .await
        }
        Ok(RecoveryActionOutcome::Satisfied { proof_kind, proof_reference }) => {
            tracing::info!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                proof_kind,
                proof_reference,
                "recovery action satisfied"
            );
            WorkerResult::Success
        }
        Ok(RecoveryActionOutcome::Failed { failure_code }) => {
            tracing::warn!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                failure_code,
                "recovery action failed"
            );
            WorkerResult::Success
        }
        Ok(RecoveryActionOutcome::ManualReconcile { failure_code }) => {
            tracing::error!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                failure_code,
                "recovery action requires manual reconcile"
            );
            WorkerResult::Success
        }
        Err(error) => {
            tracing::warn!(error = %error, "recovery action observation failed");
            release_recovery_action(store, clock, claim, retry_delay, "recovery_observation_failed")
                .await
        }
    }
}

async fn release_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    claim: auction_application::RecoveryActionClaim,
    retry_delay: ChronoDuration,
    failure_code: &'static str,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let now = clock.now();
    let retry_at = now.checked_add_signed(retry_delay).unwrap_or(now);
    if let Err(error) = store.release_action(claim, retry_at, failure_code.to_owned()).await {
        tracing::error!(error = %error, failure_code, "recovery action retry fence failed");
        return WorkerResult::Error;
    }
    WorkerResult::Success
}

async fn process_claim<C>(
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
        cancel_reason: None,
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

async fn process_delivery_claim<C>(
    delivery_store: Arc<dyn DeliveryWorkStore>,
    delivery_transport: Arc<dyn DeliveryTransport>,
    clock: Arc<C>,
    config: DeliveryWorkerRuntimeConfig,
    claim: DeliveryWorkClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if let Err(error) = delivery_store.start_sending(claim.clone(), clock.now()).await {
        tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker failed to start sending");
        // 外部写尚未开始，尽力释放 claim；失败时仍由 lease 到期后的安全领取兜底。
        match tokio::time::timeout(
            config.finalize_timeout,
            delivery_store.release(claim.clone(), clock.now()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(release_error)) => {
                tracing::error!(error = %release_error, delivery_id = %claim.delivery_id, "delivery worker failed to release claim after start failure");
            }
            Err(_) => {
                tracing::error!(delivery_id = %claim.delivery_id, "delivery worker timed out while releasing claim after start failure");
            }
        }
        return WorkerResult::Error;
    }
    let Ok(lease_span) = ChronoDuration::from_std(config.lease_duration) else {
        tracing::error!(delivery_id = %claim.delivery_id, "delivery worker configured lease span is invalid");
        return WorkerResult::Error;
    };
    let mut renew_tick = tokio::time::interval(config.renew_interval);
    renew_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // tokio interval 的首个 tick 立即完成，先消费它，避免刚进入 sending 就做无意义续租。
    renew_tick.tick().await;
    let request_timeout = tokio::time::sleep(config.request_timeout);
    tokio::pin!(request_timeout);
    let mut send = delivery_transport.send(claim.clone());
    let mut lease_failed = false;
    let result = loop {
        tokio::select! {
            result = &mut send => break result,
            _ = &mut request_timeout => {
                break Err(delivery_gateway_outcome_unknown(
                    "request_timeout",
                    "delivery 外部请求超时，远端结果未知",
                ));
            }
            _ = renew_tick.tick() => {
                let renewed_at = clock.now();
                let Some(lease_until) = renewed_at.checked_add_signed(lease_span) else {
                    tracing::error!(delivery_id = %claim.delivery_id, "delivery worker renewal deadline overflowed");
                    lease_failed = true;
                    break Err(delivery_gateway_outcome_unknown(
                        "lease_renewal",
                        "delivery 发送期间 lease 续租截止时间溢出",
                    ));
                };
                match tokio::time::timeout(
                    config.finalize_timeout,
                    delivery_store.renew(claim.clone(), renewed_at, lease_until),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::error!(error = %error, delivery_id = %claim.delivery_id, "delivery worker lost lease while sending");
                        lease_failed = true;
                        break Err(delivery_gateway_outcome_unknown(
                            "lease_renewal",
                            "delivery 发送期间 lease 续租失败",
                        ));
                    }
                    Err(_) => {
                        tracing::error!(delivery_id = %claim.delivery_id, "delivery worker lease renewal timed out");
                        lease_failed = true;
                        break Err(delivery_gateway_outcome_unknown(
                            "lease_renewal_timeout",
                            "delivery 发送期间 lease 续租超时",
                        ));
                    }
                }
            }
        }
    };
    let delivery_id = claim.delivery_id;
    match result {
        Ok(result) => {
            match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.succeed(claim, result, clock.now()),
            )
            .await
            {
                Ok(Ok(())) if lease_failed => WorkerResult::Error,
                Ok(Ok(())) => WorkerResult::Success,
                Ok(Err(error)) => {
                    tracing::error!(error = %error, %delivery_id, "delivery worker failed to persist success");
                    WorkerResult::Error
                }
                Err(_) => {
                    tracing::error!(%delivery_id, "delivery worker timed out while persisting success");
                    WorkerResult::Error
                }
            }
        }
        Err(error) => {
            let failure_code = error.failure_code.clone();
            match tokio::time::timeout(
                config.finalize_timeout,
                delivery_store.fail(claim, error, clock.now()),
            )
            .await
            {
                Ok(Ok(())) if lease_failed => WorkerResult::Error,
                Ok(Ok(())) => WorkerResult::Success,
                Ok(Err(persist_error)) => {
                    tracing::error!(error = %persist_error, %delivery_id, %failure_code, "delivery worker failed to persist failure");
                    WorkerResult::Error
                }
                Err(_) => {
                    tracing::error!(%delivery_id, %failure_code, "delivery worker timed out while persisting failure");
                    WorkerResult::Error
                }
            }
        }
    }
}

fn delivery_gateway_outcome_unknown(
    stage: &'static str,
    safe_detail: &'static str,
) -> DeliveryTransportError {
    DeliveryTransportError {
        failure_code: "gateway_outcome_unknown".to_owned(),
        safe_detail: safe_detail.to_owned(),
        outcome_unknown: true,
        external_reference: None,
        result_metadata: serde_json::json!({
            "category": "gateway_error",
            "stage": stage,
        }),
    }
}

async fn record_fulfillment_failure<C>(
    fulfillment_store: Arc<dyn SettlementFulfillmentStore>,
    clock: Arc<C>,
    claim: SettlementFulfillmentClaim,
    retry_delay: ChronoDuration,
    failure_code: &str,
    safe_detail: String,
) where
    C: Clock + 'static,
{
    let retry_at = clock.now().checked_add_signed(retry_delay).unwrap_or_else(|| clock.now());
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
    due_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct RecoveryPaymentReminderRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: String,
    payment_reference: String,
}

#[derive(Debug, Serialize)]
struct RecoveryTakeRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: String,
    source_external_mail_id: String,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RecoveryActivationRequest {
    action_id: uuid::Uuid,
    delivery_id: uuid::Uuid,
    prepare_command_id: uuid::Uuid,
    prepare_request_hash: [u8; 32],
}

#[derive(Debug, Serialize)]
struct RecoveryBuyerBlacklistRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    buyer_user_id: uuid::Uuid,
    public_reason: &'static str,
    actor_id: &'static str,
    occurred_at: chrono::DateTime<chrono::Utc>,
}

fn retry_delay(attempt_count: u32) -> ChronoDuration {
    let exponent = attempt_count.saturating_sub(1).min(8);
    let seconds = 5_u64.saturating_mul(1_u64 << exponent).min(15 * 60);
    ChronoDuration::seconds(i64::try_from(seconds).unwrap_or(15 * 60))
}

/// 合并批次内任务结果；取消优先于错误，错误优先于成功。
fn merge_worker_result(current: &mut WorkerResult, next: WorkerResult) {
    match next {
        WorkerResult::Canceled => *current = WorkerResult::Canceled,
        WorkerResult::Error if *current == WorkerResult::Success => {
            *current = WorkerResult::Error;
        }
        WorkerResult::Success | WorkerResult::Error => {}
    }
}

fn bind_worker_metric_modes(recorder: &dyn WorkerRecorder) {
    for loop_kind in [WorkerLoop::MailMain, WorkerLoop::MailIsekai] {
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Inline);
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Persisted);
    }
    for loop_kind in
        [WorkerLoop::Delivery, WorkerLoop::AuctionFulfillment, WorkerLoop::AuctionRecovery]
    {
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Persisted);
    }
}
