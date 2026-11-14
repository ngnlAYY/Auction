//! PostgreSQL 适配器的共享状态、迁移边界与进程 readiness。

use std::sync::Arc;

use auction_application::core::ApplicationError;
use auction_application::operations::ProcessReadinessPort;
use auction_observability::{HeartbeatHealthReader, HeartbeatReadinessFuture, PoolMetricsSource};
use sqlx::{PgPool, Postgres, Row};

use crate::database::backend::{DatabaseBackend, StorageError};
use crate::database::pool::{PostgresPoolMetricsSource, TrackedPool};
use crate::database::profiles::DatabaseConnectionProfile;
use crate::database::{migrations, operations::audit_health};

/// PostgreSQL 适配器共享状态；连接池供各功能 owner 的仓储和事务实现复用。
#[derive(Debug, Clone)]
pub struct DatabaseStore {
    pub(super) pool: TrackedPool<Postgres>,
    pub(crate) transaction_owner: auction_observability::DatabaseTransactionOwner,
    pub(crate) delivery_send_intervals:
        std::sync::Arc<std::sync::RwLock<auction_application::delivery::DeliverySendIntervals>>,
    pub(crate) runtime_switches: auction_application::runtime_switches::RuntimeSwitches,
    /// 告警读侧影子比对记录端口；默认 no-op，装配根注入进程指标记录器。
    pub(crate) shadow_recorder: std::sync::Arc<dyn auction_application::shadow_run::ShadowRecorder>,
    pub(crate) migration_compatibility_mode:
        auction_application::operations::MigrationCompatibilityMode,
    pub(crate) migration_readiness_recorder: auction_observability::MigrationReadinessRecorder,
    /// 通知生命周期观测；默认 no-op，装配根在 Worker 启动时注入进程记录器。
    pub(crate) notification_metrics:
        std::sync::Arc<dyn auction_application::notification_metrics::NotificationRecorder>,
    /// 通知正文使用的公开站点 origin；装配根注入，未配置时正文省略链接。
    pub(crate) public_base_url: Option<String>,
    /// 通知正文模板的进程内快照；装配根启动周期刷新，创建被超提醒时取当前快照渲染并冻结，
    /// 发送阶段不再读取。所有 store clone 共享同一份快照。
    pub(crate) mail_templates: auction_application::mail_templates::LiveMailTemplates,
    /// 同一拍卖的出价队列（进程内按场次互斥 + 跨实例会话锁）；进程内共享，clone 复用同一状态。
    pub(crate) bid_queue: std::sync::Arc<crate::database::auction::competition::AuctionBidQueue>,
}

impl DatabaseStore {
    /// 连接 PostgreSQL 并构造共享适配器；非 PostgreSQL URL 在连接前失败关闭。
    pub async fn connect(
        database_url: &str,
        profile: DatabaseConnectionProfile,
    ) -> Result<Self, StorageError> {
        let backend = DatabaseBackend::from_url(database_url)?;
        let options = profile
            .connect_options(database_url)
            .map_err(|source| StorageError::Connection { backend, source })?;
        let ordinary_capacity = profile.ordinary_capacity();
        let pool = profile
            .pool_options()
            .connect_with(options)
            .await
            .map_err(|source| StorageError::Connection { backend, source })?;

        Ok(Self {
            pool: TrackedPool::new(pool, ordinary_capacity),
            transaction_owner: auction_observability::DatabaseTransactionOwner::Admin,
            delivery_send_intervals: std::sync::Arc::new(std::sync::RwLock::new(
                auction_application::delivery::DeliverySendIntervals::default(),
            )),
            runtime_switches: auction_application::runtime_switches::RuntimeSwitches::default(),
            shadow_recorder: auction_application::shadow_run::noop_shadow_recorder(),
            migration_compatibility_mode:
                auction_application::operations::MigrationCompatibilityMode::default(),
            migration_readiness_recorder:
                auction_observability::MigrationReadinessRecorder::default(),
            notification_metrics:
                auction_application::notification_metrics::noop_notification_recorder(),
            public_base_url: None,
            mail_templates: auction_application::mail_templates::LiveMailTemplates::default(),
            bid_queue: std::sync::Arc::new(
                crate::database::auction::competition::AuctionBidQueue::default(),
            ),
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

    /// 当前进程共享的动态开关；告警读侧每次读取前解析一次口径。
    pub(crate) fn runtime_switches(
        &self,
    ) -> &auction_application::runtime_switches::RuntimeSwitches {
        &self.runtime_switches
    }

    /// 影子比对记录器；关闭指标时仍是同一 no-op，不改变比较路径。
    pub(crate) fn shadow_recorder(&self) -> &dyn auction_application::shadow_run::ShadowRecorder {
        self.shadow_recorder.as_ref()
    }

    /// 当前生效的发送节奏；热读取成功后由装配根原子替换，claim 事务每次读取快照。
    pub(crate) fn delivery_send_intervals(
        &self,
    ) -> auction_application::delivery::DeliverySendIntervals {
        *self.delivery_send_intervals.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 当前进程的通知指标记录器；关闭指标时为 no-op，不影响入队、抑制或退役结果。
    pub(crate) fn notification_metrics(
        &self,
    ) -> &dyn auction_application::notification_metrics::NotificationRecorder {
        self.notification_metrics.as_ref()
    }

    /// 当前生效的出价被超通知模板；只在创建通知时读取，发送阶段使用冻结正文。返回的快照
    /// 不随之后的刷新变化，同一事务内创建的通知用同一套模板。
    pub(crate) fn mail_templates(
        &self,
    ) -> std::sync::Arc<auction_application::mail_templates::MailTemplateSet> {
        self.mail_templates.current()
    }

    pub(crate) fn raw_pool(&self) -> &PgPool {
        self.pool.raw()
    }

    /// 出价队列的共享状态；同一进程内所有 store clone 共用同一组场次互斥量。
    pub(crate) fn auction_bid_queue(
        &self,
    ) -> &std::sync::Arc<crate::database::auction::competition::AuctionBidQueue> {
        &self.bid_queue
    }

    /// 指标关闭只使用 no-op，不能改变事务或连接释放结果。
    pub fn with_database_recorder(
        mut self,
        recorder: auction_observability::DatabaseRecorder,
    ) -> Self {
        self.pool = self.pool.clone().with_database_recorder(recorder);
        self
    }

    pub(crate) fn tracked_postgres_pool(&self) -> &TrackedPool<Postgres> {
        &self.pool
    }

    /// 装配根注入迁移就绪模式；只放宽探针，不改变迁移、备份与恢复的严格边界。
    pub fn with_migration_compatibility_mode(
        mut self,
        mode: auction_application::operations::MigrationCompatibilityMode,
    ) -> Self {
        self.migration_compatibility_mode = mode;
        self
    }

    /// 共享进程指标记录器；关闭指标不能改变 schema 判定。
    pub fn with_migration_readiness_recorder(
        mut self,
        recorder: auction_observability::MigrationReadinessRecorder,
    ) -> Self {
        self.migration_readiness_recorder = recorder;
        self
    }

    /// 注入通知指标记录器；只观测生命周期，不改变入队、抑制、退役或发送结果。
    pub fn with_notification_recorder(
        mut self,
        recorder: std::sync::Arc<
            dyn auction_application::notification_metrics::NotificationRecorder,
        >,
    ) -> Self {
        self.notification_metrics = recorder;
        self
    }

    /// 注入通知正文使用的公开站点 origin；只影响新通知的冻结正文，不改变已入队载荷。
    pub fn with_public_base_url(mut self, url: Option<String>) -> Self {
        self.public_base_url = url;
        self
    }

    /// 本进程共享的通知模板快照句柄。装配根把它交给周期刷新任务与 Worker 的发送路径；
    /// 快照变化只影响此后创建或渲染的通知，已冻结载荷保持不变。
    pub fn live_mail_templates(&self) -> auction_application::mail_templates::LiveMailTemplates {
        self.mail_templates.clone()
    }

    /// 注入同一进程共享的动态只读策略；不改变审计写入和事务重试。
    pub fn with_runtime_switches(
        mut self,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Self {
        self.runtime_switches = switches;
        self
    }

    /// 注入影子比对记录器；只累加闭集样本，不改变告警可见值。
    pub fn with_shadow_recorder(
        mut self,
        recorder: std::sync::Arc<dyn auction_application::shadow_run::ShadowRecorder>,
    ) -> Self {
        self.shadow_recorder = recorder;
        self
    }

    /// 兼容已有装配调用，基线仍由配置 owner 校验。
    pub fn with_audit_failure_mode(
        self,
        mode: auction_application::operations::AuditFailureMode,
    ) -> Self {
        self.with_runtime_switches(auction_application::runtime_switches::RuntimeSwitches::new(
            auction_application::runtime_switches::RuntimeSwitchRole::Worker,
            mode,
            true,
        ))
    }

    /// 指标与心跳读取同一动态策略，分类累计永不清零。
    pub fn audit_metrics_source(&self) -> Arc<dyn auction_observability::AuditMetricsSource> {
        audit_health::metrics_source(self.runtime_switches.clone())
    }

    /// 装配根在共享适配器前注入启动快照；不从存储层读取配置或环境。
    pub fn with_delivery_send_intervals(
        self,
        intervals: auction_application::delivery::DeliverySendIntervals,
    ) -> Result<Self, auction_application::core::RepositoryError> {
        self.set_delivery_send_intervals(intervals)?;
        Ok(self)
    }

    /// 热读取在完整候选校验后原子替换发送节奏；同一进程的 clone 立即看到新值。
    /// 只改变发送开始的最小间距，不改变金额、资格、幂等或既有 claim。
    pub fn set_delivery_send_intervals(
        &self,
        intervals: auction_application::delivery::DeliverySendIntervals,
    ) -> Result<(), auction_application::core::RepositoryError> {
        if !intervals.is_valid() {
            return Err(auction_application::core::RepositoryError::Failed(
                "邮件发送间隔必须位于 0 至 86400 秒".to_owned(),
            ));
        }
        *self.delivery_send_intervals.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
            intervals;
        Ok(())
    }

    /// 运行当前后端已提交的追加式迁移；仅运行 PostgreSQL 迁移，未知非空数据库仍拒绝初始化。
    pub async fn migrate(&self) -> Result<(), StorageError> {
        let marker_exists = ensure_postgres_migration_target(self.raw_pool()).await?;
        let applied_checksums = if marker_exists {
            postgres_compatibility_checksums(self.raw_pool()).await?
        } else {
            Vec::new()
        };
        let migrator = migrations::postgres_migrator_with_applied_checksums(&applied_checksums)?;
        let mut connection = self.raw_pool().acquire().await.map_err(|source| {
            StorageError::Migration { backend: DatabaseBackend::Postgres, source: source.into() }
        })?;
        // SQLx 的提前错误或取消不保证到达 unlock；专用会话不能携带迁移锁归池。
        connection.close_on_drop();
        migrator.run(&mut *connection).await.map_err(|source| StorageError::Migration {
            backend: DatabaseBackend::Postgres,
            source,
        })
    }

    /// 返回原生连接池，供装配根显式关闭连接。
    pub fn postgres_pool(&self) -> &PgPool {
        self.raw_pool()
    }

    /// 返回不含连接串和数据库名称的连接池状态适配器。
    pub fn pool_metrics_source(&self) -> Arc<dyn PoolMetricsSource> {
        Arc::new(PostgresPoolMetricsSource::new(self.pool.clone()))
    }

    /// 切换排队模式；`Direct` 只用于基准对照与回退验证，生产装配始终使用默认的 `Queued`。
    #[cfg(feature = "test-db")]
    pub fn with_bid_queue_mode(
        mut self,
        mode: crate::database::auction::competition::BidQueueMode,
    ) -> Self {
        let budget = self.bid_queue.wait_budget();
        self.bid_queue = std::sync::Arc::new(
            crate::database::auction::competition::AuctionBidQueue::new(mode, budget),
        );
        self
    }

    /// 显式关闭当前适配器共享的连接池。
    /// 恢复工具在只读预检后必须先释放自身连接，再把空库交给 `pg_restore`；普通服务进程依靠装配根
    /// 生命周期关闭，不需要调用该方法。
    pub async fn close(self) {
        self.raw_pool().close().await;
    }

    /// HTTP 与 heartbeat 共用数据库/schema 证明及模式化审计门禁。
    async fn check_process_readiness(&self) -> Result<(), ApplicationError> {
        self.check_database_readiness().await?;
        let mode = self.runtime_switches.readiness_policy().audit_failure_mode;
        if !audit_health::is_ready(mode) {
            // 门禁拒绝的根因在审计类别；不得让上层日志把它误读成数据库/schema 故障。
            tracing::warn!(
                target: "auction::storage::audit_health",
                audit_failure_mode = auction_application::runtime_switches::audit_label(mode),
                blocking_failures = audit_health::failure_count(mode),
                "审计失败门禁阻断进程 readiness"
            );
            return Err(auction_application::core::RepositoryError::Unavailable.into());
        }
        Ok(())
    }

    /// 以只读查询确认连接可用且已应用当前二进制的全部迁移。
    async fn check_database_readiness(&self) -> Result<(), ApplicationError> {
        migrations::check_postgres_readiness(self).await
    }
}

impl ProcessReadinessPort for DatabaseStore {
    fn check(&self) -> auction_application::auction::ApplicationFuture<'_, ()> {
        Box::pin(self.check_process_readiness())
    }
}

impl HeartbeatHealthReader for DatabaseStore {
    fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.check_process_readiness().await.is_ok() })
    }

    fn audit_failure_count(&self) -> i64 {
        audit_health::failure_count(self.runtime_switches.readiness_policy().audit_failure_mode)
    }
}

async fn ensure_postgres_migration_target(pool: &PgPool) -> Result<bool, StorageError> {
    let marker_exists =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await
            .map_err(|source| StorageError::MigrationTarget {
                backend: DatabaseBackend::Postgres,
                source,
            })?;
    if marker_exists {
        return Ok(true);
    }

    let user_object_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)::bigint
         FROM pg_class AS relation
         JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
         WHERE namespace.nspname NOT IN ('pg_catalog', 'information_schema')
           AND namespace.nspname NOT LIKE 'pg_toast%'
           AND relation.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')",
    )
    .fetch_one(pool)
    .await
    .map_err(|source| StorageError::MigrationTarget {
        backend: DatabaseBackend::Postgres,
        source,
    })?;
    if user_object_count > 0 {
        return Err(StorageError::UnknownDatabase { backend: DatabaseBackend::Postgres });
    }
    Ok(false)
}

async fn postgres_compatibility_checksums(
    pool: &PgPool,
) -> Result<Vec<(i64, Vec<u8>)>, StorageError> {
    let rows = sqlx::query(
        "SELECT version, checksum
         FROM _sqlx_migrations
         WHERE version IN (5, 15)
         ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|source| StorageError::MigrationTarget {
        backend: DatabaseBackend::Postgres,
        source,
    })?;
    let mut checksums = Vec::with_capacity(rows.len());
    for row in rows {
        let version = row.try_get::<i64, _>("version").map_err(|source| {
            StorageError::MigrationTarget { backend: DatabaseBackend::Postgres, source }
        })?;
        let checksum = row.try_get::<Vec<u8>, _>("checksum").map_err(|source| {
            StorageError::MigrationTarget { backend: DatabaseBackend::Postgres, source }
        })?;
        checksums.push((version, checksum));
    }
    Ok(checksums)
}

/// 返回当前二进制内嵌 PostgreSQL 迁移树的最高版本。
/// 运维恢复入口用它在 `pg_restore` 前拒绝来自未来 schema 的 archive；旧版本 archive 可恢复到
/// 空库，随后仍须通过正常前向迁移升级。
pub fn postgres_expected_migration_version() -> i64 {
    super::migrations::embedded_latest_version()
}
