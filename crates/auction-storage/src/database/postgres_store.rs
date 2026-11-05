//! PostgreSQL 连接池与迁移版本边界。

use super::*;

/// PostgreSQL 适配器共享状态；连接池供各功能 owner 的仓储和事务实现复用。
#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pub(super) pool: TrackedPool<Postgres>,
    pub(crate) transaction_owner: auction_observability::DatabaseTransactionOwner,
    pub(crate) delivery_send_intervals:
        std::sync::Arc<std::sync::RwLock<auction_application::DeliverySendIntervals>>,
    pub(crate) runtime_switches: auction_application::RuntimeSwitches,
    pub(crate) migration_compatibility_mode: auction_application::MigrationCompatibilityMode,
    pub(crate) migration_readiness_recorder: auction_observability::MigrationReadinessRecorder,
    /// W14.8 通知生命周期观测；默认 no-op，装配根在 Worker 启动时注入进程记录器。
    pub(crate) notification_metrics: std::sync::Arc<dyn auction_application::NotificationRecorder>,
    /// W14.2 通知正文使用的公开站点 origin；装配根注入，未配置时正文省略链接。
    pub(crate) public_base_url: Option<String>,
    /// 同一拍卖的出价队列（进程内按场次互斥 + 跨实例会话锁）；进程内共享，clone 复用同一状态。
    pub(crate) bid_queue: std::sync::Arc<super::competition::AuctionBidQueue>,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub async fn connect(
        database_url: &str,
        profile: DatabaseConnectionProfile,
    ) -> Result<Self, sqlx::Error> {
        let options = profile.connect_options(database_url)?;
        let ordinary_capacity = profile.ordinary_capacity();
        let pool = profile.pool_options().connect_with(options).await?;

        Ok(Self {
            // 普通业务配额小于物理池，保证 readiness 始终有可用连接。
            pool: TrackedPool::new(pool, ordinary_capacity),
            transaction_owner: auction_observability::DatabaseTransactionOwner::Admin,
            delivery_send_intervals: std::sync::Arc::new(std::sync::RwLock::new(
                auction_application::DeliverySendIntervals::default(),
            )),
            runtime_switches: auction_application::RuntimeSwitches::default(),
            migration_compatibility_mode: auction_application::MigrationCompatibilityMode::default(
            ),
            migration_readiness_recorder:
                auction_observability::MigrationReadinessRecorder::default(),
            notification_metrics: auction_application::noop_notification_recorder(),
            public_base_url: None,
            bid_queue: std::sync::Arc::new(super::competition::AuctionBidQueue::default()),
        })
    }

    /// 为共享池的命令绑定闭集观测 owner，不修改业务策略或其他 store clone。
    pub fn for_transaction(&self, owner: auction_observability::DatabaseTransactionOwner) -> Self {
        let mut store = self.clone();
        store.transaction_owner = owner;
        store
    }

    pub(crate) fn pool(&self) -> &TrackedPool<Postgres> {
        &self.pool
    }

    /// 当前生效的发送节奏；热读取成功后由装配根原子替换，claim 事务每次读取快照。
    pub(crate) fn delivery_send_intervals(&self) -> auction_application::DeliverySendIntervals {
        *self.delivery_send_intervals.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 当前进程的通知指标记录器；关闭指标时为 no-op，不影响入队、抑制或退役结果。
    pub(crate) fn notification_metrics(&self) -> &dyn auction_application::NotificationRecorder {
        self.notification_metrics.as_ref()
    }

    pub(crate) fn raw_pool(&self) -> &PgPool {
        self.pool.raw()
    }

    /// 出价队列的共享状态；同一进程内所有 store clone 共用同一组场次互斥量。
    pub(crate) fn auction_bid_queue(&self) -> &std::sync::Arc<super::competition::AuctionBidQueue> {
        &self.bid_queue
    }

    /// 切换排队模式；`Direct` 只用于基准对照与回退验证，生产装配始终使用默认的 `Queued`。
    #[cfg(feature = "test-db")]
    pub fn with_bid_queue_mode(mut self, mode: super::competition::BidQueueMode) -> Self {
        let budget = self.bid_queue.wait_budget();
        self.bid_queue =
            std::sync::Arc::new(super::competition::AuctionBidQueue::new(mode, budget));
        self
    }

    /// 显式关闭当前适配器共享的连接池。
    /// 恢复工具在只读预检后必须先释放自身连接，再把空库交给 `pg_restore`；普通服务进程依靠装配根
    /// 生命周期关闭，不需要调用该方法。
    pub async fn close(self) {
        self.raw_pool().close().await;
    }
}

#[cfg(feature = "postgres")]
/// 返回当前二进制内嵌 PostgreSQL 迁移树的最高版本。
/// 运维恢复入口用它在 `pg_restore` 前拒绝来自未来 schema 的 archive；旧版本 archive 可恢复到
/// 空库，随后仍须通过正常前向迁移升级。
pub fn postgres_expected_migration_version() -> i64 {
    sqlx::migrate!("../../migrations/postgres")
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}
