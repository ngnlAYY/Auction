//! 运行时数据库选择、迁移保护和进程 readiness。

use super::*;

/// 运行时选择的数据库边界。领域层和应用层不得依赖该枚举，只能使用仓储与事务端口。
#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub enum DatabaseStore {
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
}

#[cfg(feature = "postgres")]
impl DatabaseStore {
    /// 指标关闭只使用 no-op，不能改变事务或连接释放结果。
    pub fn with_database_recorder(
        mut self,
        recorder: auction_observability::DatabaseRecorder,
    ) -> Self {
        let Self::Postgres(store) = &mut self;
        store.pool = store.pool.clone().with_database_recorder(recorder);
        self
    }

    pub(crate) fn tracked_postgres_pool(&self) -> &TrackedPool<Postgres> {
        let Self::Postgres(store) = self;
        store.pool()
    }

    /// 装配根注入迁移就绪模式；只放宽探针，不改变迁移、备份与恢复的严格边界。
    pub fn with_migration_compatibility_mode(
        mut self,
        mode: auction_application::MigrationCompatibilityMode,
    ) -> Self {
        let Self::Postgres(store) = &mut self;
        store.migration_compatibility_mode = mode;
        self
    }

    /// 共享进程指标记录器；关闭指标不能改变 schema 判定。
    pub fn with_migration_readiness_recorder(
        mut self,
        recorder: auction_observability::MigrationReadinessRecorder,
    ) -> Self {
        let Self::Postgres(store) = &mut self;
        store.migration_readiness_recorder = recorder;
        self
    }

    /// 注入同一进程共享的动态只读策略；不改变审计写入和事务重试。
    pub fn with_runtime_switches(mut self, switches: auction_application::RuntimeSwitches) -> Self {
        let Self::Postgres(store) = &mut self;
        store.runtime_switches = switches;
        self
    }

    /// 兼容已有装配调用，基线仍由配置 owner 校验。
    pub fn with_audit_failure_mode(self, mode: auction_application::AuditFailureMode) -> Self {
        self.with_runtime_switches(auction_application::RuntimeSwitches::new(
            auction_application::RuntimeSwitchRole::Worker,
            mode,
            true,
        ))
    }

    /// 指标与心跳读取同一动态策略，分类累计永不清零。
    pub fn audit_metrics_source(&self) -> Arc<dyn auction_observability::AuditMetricsSource> {
        let Self::Postgres(store) = self;
        audit_health::metrics_source(store.runtime_switches.clone())
    }

    pub async fn connect(
        database_url: &str,
        profile: DatabaseConnectionProfile,
    ) -> Result<Self, StorageError> {
        let backend = DatabaseBackend::from_url(database_url)?;

        PostgresStore::connect(database_url, profile)
            .await
            .map(Self::Postgres)
            .map_err(|source| StorageError::Connection { backend, source })
    }

    /// 装配根在共享适配器前注入启动快照；不从存储层读取配置或环境。
    pub fn with_delivery_send_intervals(
        mut self,
        intervals: auction_application::DeliverySendIntervals,
    ) -> Result<Self, auction_application::RepositoryError> {
        if !intervals.is_valid() {
            return Err(auction_application::RepositoryError::Failed(
                "邮件发送间隔必须位于 0 至 86400 秒".to_owned(),
            ));
        }
        let Self::Postgres(store) = &mut self;
        store.delivery_send_intervals = intervals;
        Ok(self)
    }

    pub const fn backend(&self) -> DatabaseBackend {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => DatabaseBackend::Postgres,
        }
    }

    /// 运行当前后端已提交的追加式迁移；仅运行 PostgreSQL 迁移，未知非空数据库仍拒绝初始化。
    pub async fn migrate(&self) -> Result<(), StorageError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                let marker_exists = ensure_postgres_migration_target(store.raw_pool()).await?;
                let applied_checksums = if marker_exists {
                    postgres_compatibility_checksums(store.raw_pool()).await?
                } else {
                    Vec::new()
                };
                let migrator =
                    migrations::postgres_migrator_with_applied_checksums(&applied_checksums)?;
                let mut connection =
                    store.raw_pool().acquire().await.map_err(|source| StorageError::Migration {
                        backend: DatabaseBackend::Postgres,
                        source: source.into(),
                    })?;
                // SQLx 的提前错误或取消不保证到达 unlock；专用会话不能携带迁移锁归池。
                connection.close_on_drop();
                migrator.run(&mut *connection).await.map_err(|source| StorageError::Migration {
                    backend: DatabaseBackend::Postgres,
                    source,
                })
            }
        }
    }

    #[cfg(feature = "postgres")]
    /// 返回原生连接池，供装配根显式关闭连接。
    pub fn postgres_pool(&self) -> &PgPool {
        let Self::Postgres(store) = self;
        store.raw_pool()
    }

    /// 返回不含连接串和数据库名称的连接池状态适配器。
    pub fn pool_metrics_source(&self) -> Arc<dyn PoolMetricsSource> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => Arc::new(PostgresPoolMetricsSource::new(store.pool().clone())),
        }
    }
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
impl ProcessReadinessPort for DatabaseStore {
    fn check(&self) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(self.check_process_readiness())
    }
}

#[cfg(feature = "postgres")]
impl HeartbeatHealthReader for DatabaseStore {
    fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.check_process_readiness().await.is_ok() })
    }

    fn audit_failure_count(&self) -> i64 {
        let Self::Postgres(store) = self;
        audit_health::failure_count(store.runtime_switches.readiness_policy().audit_failure_mode)
    }
}

#[cfg(feature = "postgres")]
impl DatabaseStore {
    /// HTTP 与 heartbeat 共用数据库/schema 证明及模式化审计门禁。
    async fn check_process_readiness(&self) -> Result<(), ApplicationError> {
        self.check_database_readiness().await?;
        let Self::Postgres(store) = self;
        if !audit_health::is_ready(store.runtime_switches.readiness_policy().audit_failure_mode) {
            return Err(storage_error("审计失败阻断进程 readiness"));
        }
        Ok(())
    }

    /// 以只读查询确认连接可用且已应用当前二进制的全部迁移。
    async fn check_database_readiness(&self) -> Result<(), ApplicationError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => migrations::check_postgres_readiness(store).await,
        }
    }
}
