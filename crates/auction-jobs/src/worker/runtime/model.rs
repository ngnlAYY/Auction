//! Worker 持久化运行时的端口与运行参数模型。

use super::*;

/// 持久化 Worker 的端口与运行参数。
///
/// PostgreSQL 完整档可以同时提供结算与履约端口；SQLite Lite 将这些完整档端口保持为 `None`，
/// 由装配根显式表达能力差异。
pub struct PersistentWorker<C> {
    pub schedule: Arc<dyn ScheduledRoundStore>,
    /// 周计划每轮 bootstrap 前读取的数据库业务世界权威状态。
    pub worlds: Option<Arc<dyn WorldReader>>,
    pub lifecycle: Arc<dyn AuctionLifecycleCommandPort>,
    pub slow_proxy_due: Option<Arc<dyn SlowProxyDueReader>>,
    pub slow_proxy_commands: Option<Arc<dyn SlowProxyCommandPort>>,
    pub auction: AuctionWorkerRuntimeConfig,
    /// 当前周 bootstrap 与 Mail 暂停/恢复所需的持久化调度配置。
    pub schedule_runtime: Option<AuctionScheduleRuntimeConfig>,
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
