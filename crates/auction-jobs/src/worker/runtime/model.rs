//! Worker 持久化运行时的端口与运行参数模型。

use super::*;

/// 持久化 Worker 的端口与运行参数。
/// 装配根显式注入结算与履约端口，未配置的任务不可作为成功执行。
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
    /// W11.1–W11.3 资产对账：只追加报告事实，不授权资金或资产动作。
    pub asset_reconciliation: Option<AssetReconciliationRuntime>,
    pub recovery_store: Option<Arc<dyn RecoveryWorkStore>>,
    pub recovery_resolution: Option<Arc<dyn RecoveryResolutionStore>>,
    pub recovery_delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    pub recovery_blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    pub recovery: RecoveryWorkerRuntimeConfig,
    pub delivery_store: Option<Arc<dyn DeliveryWorkStore>>,
    pub delivery_transport: Option<Arc<dyn DeliveryTransport>>,
    pub delivery: DeliveryWorkerRuntimeConfig,
    /// runtime 文件的周期热读取快照；循环在每个边界读取完整快照。
    pub hot: WorkerHotRuntime,
    /// 每个启用的 MoogleMail world 都拥有独立的持久化游标和 gateway。
    pub mail_polls: Vec<mail_poll::MailPollRuntime>,
    pub forum: ForumWorkerRuntimeConfig,
    pub forum_sync: Option<forum_sync::ForumSyncRuntime>,
    pub forum_reply: Option<forum_reply::ForumReplyRuntime>,
    /// 可选 operations alert 扫描器；只在 PostgreSQL 完整档和明确启用时注入。
    pub operations_alert: Option<Arc<operations_alert::OperationsAlertRuntime>>,
    /// W14.3 卖方汇总与换季提醒扫描；只读事实并幂等入队，发送仍由 delivery 循环负责。
    pub player_notifications: Option<Arc<player_notifications::PlayerNotificationRuntime>>,
    /// W12.1 残留：出价提交后的标记/被超提醒 outbox 扫描；未注入时不处理，事件保持持久待处理。
    pub lot_watch_reminders: Option<Arc<dyn auction_application::LotWatchReminderStore>>,
    /// Worker 固定 label 的指标端口；关闭指标时由装配根注入 no-op 实现。
    pub metrics: Arc<dyn WorkerRecorder>,
    /// W14.8 通知生命周期与送达延迟指标；关闭指标时为 no-op，不影响发送结果。
    pub notifications: Arc<dyn auction_application::NotificationRecorder>,
    /// Worker control 的进程状态和 MoogleMail 即时唤醒器。
    pub control: WorkerControlHandle,
    pub clock: Arc<C>,
    pub worker_id: String,
    /// 仅控制进程内取消；持久状态仍由原窄端口拥有。
    pub write_lifecycle: auction_application::ExternalWriteLifecycle,
    /// W12.5 批量与维护循环共享的连接配额；关键循环从不获取，未注入时退化为不限制。
    pub bulk_quota: Option<Arc<super::WorkerConnectionQuota>>,
    /// W1.9 主动暂停的共享快照；与 MoogleMail 轮询器使用同一份数据库事实。
    pub switches: auction_application::RuntimeSwitches,
}

impl<C> PersistentWorker<C> {
    /// 当前 delivery 热参数；组合后越界时保留启动配置并告警，不中断循环。
    pub(super) fn delivery_runtime(&self) -> DeliveryWorkerRuntimeConfig {
        let hot = self.delivery.with_hot(&self.hot.snapshot().delivery);
        if hot.validate() {
            hot
        } else {
            tracing::warn!("热读取的 delivery 预算与启动租约不一致，继续使用启动配置");
            self.delivery
        }
    }

    /// 批量/维护轮次的连接配额许可；许可存活期间占用配额，Drop 即释放。
    /// 关键循环（生命周期、慢速代理、独立结算、结算履约）不调用本方法。
    pub(crate) async fn bulk_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        match &self.bulk_quota {
            Some(quota) => Some(quota.acquire().await),
            None => None,
        }
    }

    /// W1.9 外部访问是否处于主动暂停；每次判定都重读最新事实与期限。
    /// 暂停只阻止开始新的外部请求，已进入 sending 围栏的写入按其自身预算完成。
    pub(crate) fn external_access_paused(&self) -> bool {
        self.switches.external_access_paused()
    }

    /// 当前论坛轮询/抖动热参数；租约仍来自启动配置。
    pub(super) fn forum_runtime(&self) -> ForumWorkerRuntimeConfig {
        let hot = self.forum.with_hot(&self.hot.snapshot().forum);
        if hot.validate() {
            hot
        } else {
            tracing::warn!("热读取的论坛轮询参数无效，继续使用启动配置");
            self.forum
        }
    }
}
