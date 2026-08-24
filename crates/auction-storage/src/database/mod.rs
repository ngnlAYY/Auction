use std::fmt;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::sync::Arc;
#[cfg(feature = "sqlite")]
use std::time::Duration;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{ApplicationError, ProcessReadinessPort, RepositoryError};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{
    HeartbeatHealthReader, HeartbeatReadinessFuture, PoolMetricsBackend, PoolMetricsSnapshot,
    PoolMetricsSource,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use sqlx::Row;
use thiserror::Error;

#[cfg(feature = "postgres")]
use sqlx::{PgPool, Postgres, postgres::PgPoolOptions};
#[cfg(feature = "postgres")]
mod admin_intake;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_mail;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod audit_health;
#[cfg(feature = "postgres")]
mod competition;
#[cfg(feature = "postgres")]
mod legacy_v28;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod mail_delivery;
#[cfg(feature = "postgres")]
mod mail_intake;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod mail_poll;
#[cfg(feature = "postgres")]
mod rebuild;
#[cfg(feature = "postgres")]
mod recovery_preflight;
#[cfg(feature = "postgres")]
mod settlement;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_report;
#[cfg(feature = "postgres")]
pub use competition::PostgresCompetitionTransaction;
#[cfg(feature = "postgres")]
pub use legacy_v28::{
    LEGACY_V28_MIGRATION_VERSION, POSTGRES_LEGACY_V28_PREFLIGHT_FORMAT, PostgresLegacyV28Adoption,
    PostgresLegacyV28Error, PostgresLegacyV28Preflight,
};
#[cfg(feature = "postgres")]
pub use rebuild::{
    POSTGRES_REBUILD_PREFLIGHT_FORMAT, PostgresDatabaseRebuildGuard, PostgresRebuildError,
    PostgresRebuildPreflight, PostgresRebuildResult,
};
#[cfg(feature = "postgres")]
pub use recovery_preflight::{
    PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError,
};
#[cfg(feature = "postgres")]
pub use retention_inventory::{
    PostgresForeignKeyCoverage, PostgresForeignKeyInventory, PostgresForeignKeyInventoryReport,
    PostgresMigrationInventory, PostgresRealtimeCoverage, PostgresRealtimeInventory,
    PostgresRealtimeStreamInventory, PostgresRelationInventory, PostgresRetentionFamilyInventory,
    PostgresRetentionInventory, PostgresRetentionTableInventory, PostgresSchemaIdentity,
    RETENTION_INVENTORY_FORMAT, RetentionInventoryError, RetentionPolicy, RetentionPolicyFamily,
};
#[cfg(feature = "sqlite")]
mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteCompetitionTransaction;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod compensation;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod credentials;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub use commands::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod account;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_auctions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_audit;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_lots;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_proxy_events;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_renames;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod auctions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod bid_log;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod blacklist;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod delivery_admin;
#[cfg(feature = "postgres")]
mod delivery_audit;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod delivery_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod forum;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod fulfillment_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod health;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod lots;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod operations_alert;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod price_history;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime_admin;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime_consumer;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_health;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_report;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_scheduler;
#[cfg(feature = "postgres")]
mod retention_inventory;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod rules;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod sessions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub use settlement_commands::DatabaseSettlementCommands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod users;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod worlds;
#[cfg(feature = "sqlite")]
use sqlx::{
    Sqlite, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod pool;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use pool::TrackedPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseBackend {
    Postgres,
    Sqlite,
}

impl DatabaseBackend {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }

    pub fn from_url(database_url: &str) -> Result<Self, StorageError> {
        let scheme = database_url.split(':').next().unwrap_or_default().to_ascii_lowercase();

        match scheme.as_str() {
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "sqlite" => Ok(Self::Sqlite),
            _ => Err(StorageError::UnsupportedScheme(scheme)),
        }
    }
}

impl fmt::Display for DatabaseBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error(
        "unsupported database URL scheme {0:?}; expected postgres://, postgresql://, or sqlite:"
    )]
    UnsupportedScheme(String),

    #[error("failed to connect to {backend} database")]
    Connection {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("{backend} storage support is not compiled; enable its Cargo feature")]
    BackendNotCompiled { backend: DatabaseBackend },

    #[error("failed to migrate {backend} database")]
    Migration {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("failed to inspect {backend} migration target")]
    MigrationTarget {
        backend: DatabaseBackend,
        #[source]
        source: sqlx::Error,
    },

    #[error("refusing to migrate an unknown non-empty {backend} database")]
    UnknownDatabase { backend: DatabaseBackend },

    #[error("migrations are not available for the {backend} backend yet")]
    MigrationsUnavailable { backend: DatabaseBackend },
}

/// 运行时选择的数据库边界。领域层和应用层不得依赖该枚举，只能使用仓储与事务端口。
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[derive(Debug, Clone)]
pub enum DatabaseStore {
    #[cfg(feature = "postgres")]
    Postgres(PostgresStore),
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteStore),
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl DatabaseStore {
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let backend = DatabaseBackend::from_url(database_url)?;

        match backend {
            DatabaseBackend::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    PostgresStore::connect(database_url)
                        .await
                        .map(Self::Postgres)
                        .map_err(|source| StorageError::Connection { backend, source })
                }

                #[cfg(not(feature = "postgres"))]
                {
                    Err(StorageError::BackendNotCompiled { backend })
                }
            }
            DatabaseBackend::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    SqliteStore::connect(database_url)
                        .await
                        .map(Self::Sqlite)
                        .map_err(|source| StorageError::Connection { backend, source })
                }

                #[cfg(not(feature = "sqlite"))]
                {
                    Err(StorageError::BackendNotCompiled { backend })
                }
            }
        }
    }

    pub const fn backend(&self) -> DatabaseBackend {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => DatabaseBackend::Postgres,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => DatabaseBackend::Sqlite,
        }
    }

    /// 运行当前后端已提交的追加式迁移。
    ///
    /// PostgreSQL 运行完整档迁移；SQLite 只运行明确标记为 Lite/共有能力的子集。
    pub async fn migrate(&self) -> Result<(), StorageError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                ensure_postgres_migration_target(store.raw_pool()).await?;
                sqlx::migrate!("../../migrations/postgres").run(store.raw_pool()).await.map_err(
                    |source| StorageError::Migration { backend: DatabaseBackend::Postgres, source },
                )
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => {
                ensure_sqlite_migration_target(store.raw_pool()).await?;
                sqlx::migrate!("../../migrations/sqlite").run(store.raw_pool()).await.map_err(
                    |source| StorageError::Migration { backend: DatabaseBackend::Sqlite, source },
                )
            }
        }
    }

    #[cfg(feature = "postgres")]
    pub fn postgres_pool(&self) -> Option<&PgPool> {
        match self {
            Self::Postgres(store) => Some(store.raw_pool()),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => None,
        }
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite_pool(&self) -> Option<&SqlitePool> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => None,
            Self::Sqlite(store) => Some(store.raw_pool()),
        }
    }

    /// 返回不含连接串和数据库名称的连接池状态适配器。
    pub fn pool_metrics_source(&self) -> Arc<dyn PoolMetricsSource> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => {
                Arc::new(PostgresPoolMetricsSource { pool: store.pool.clone() })
            }
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => Arc::new(SqlitePoolMetricsSource { pool: store.pool.clone() }),
        }
    }
}

#[cfg(feature = "postgres")]
async fn ensure_postgres_migration_target(pool: &PgPool) -> Result<(), StorageError> {
    let marker_exists =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await
            .map_err(|source| StorageError::MigrationTarget {
                backend: DatabaseBackend::Postgres,
                source,
            })?;
    if marker_exists {
        return Ok(());
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
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn ensure_sqlite_migration_target(pool: &SqlitePool) -> Result<(), StorageError> {
    let marker_exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM sqlite_master
         WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(pool)
    .await
    .map_err(|source| StorageError::MigrationTarget { backend: DatabaseBackend::Sqlite, source })?
        > 0;
    if marker_exists {
        return Ok(());
    }

    let user_object_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM sqlite_master
         WHERE type IN ('table', 'index', 'view', 'trigger')
           AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(pool)
    .await
    .map_err(|source| StorageError::MigrationTarget { backend: DatabaseBackend::Sqlite, source })?;
    if user_object_count > 0 {
        return Err(StorageError::UnknownDatabase { backend: DatabaseBackend::Sqlite });
    }
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl ProcessReadinessPort for DatabaseStore {
    fn check(&self) -> auction_application::ApplicationFuture<'_, ()> {
        Box::pin(async move {
            self.check_process_readiness().await.map_err(|_| {
                ApplicationError::Repository(RepositoryError::Failed(
                    "进程 readiness 检查失败".to_owned(),
                ))
            })
        })
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl HeartbeatHealthReader for DatabaseStore {
    fn readiness(&self) -> HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.check_process_readiness().await.is_ok() })
    }

    fn audit_failure_count(&self) -> i64 {
        audit_health::failure_count()
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
impl DatabaseStore {
    /// 复用 HTTP 与 heartbeat 的保守进程 readiness：数据库/schema 正常且没有审计写入失败。
    async fn check_process_readiness(&self) -> Result<(), ()> {
        self.check_database_readiness().await?;
        if audit_health::failure_count() > 0 {
            return Err(());
        }
        Ok(())
    }

    /// 以只读查询确认连接可用且已应用当前二进制的全部迁移。
    async fn check_database_readiness(&self) -> Result<(), ()> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => check_postgres_readiness(store).await,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => check_sqlite_readiness(store).await,
        }
    }
}

#[cfg(feature = "postgres")]
async fn check_postgres_readiness(store: &PostgresStore) -> Result<(), ()> {
    sqlx::query("SELECT 1").execute(store.raw_pool()).await.map_err(|_| ())?;
    let rows = sqlx::query("SELECT version, success FROM _sqlx_migrations ORDER BY version ASC")
        .fetch_all(store.raw_pool())
        .await
        .map_err(|_| ())?;
    let expected = sqlx::migrate!("../../migrations/postgres")
        .iter()
        .map(|migration| (migration.version, migration.checksum.as_ref().to_vec()))
        .collect::<Vec<_>>();
    if rows.len() != expected.len() {
        return Err(());
    }
    for (row, (expected_version, expected_checksum)) in rows.iter().zip(expected) {
        let version = row.try_get::<i64, _>("version").map_err(|_| ())?;
        let success = row.try_get::<bool, _>("success").map_err(|_| ())?;
        let checksum = row.try_get::<Vec<u8>, _>("checksum").map_err(|_| ())?;
        if !success || version != expected_version || checksum != expected_checksum {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn check_sqlite_readiness(store: &SqliteStore) -> Result<(), ()> {
    sqlx::query("SELECT 1").execute(store.raw_pool()).await.map_err(|_| ())?;
    let rows = sqlx::query("SELECT version, success FROM _sqlx_migrations ORDER BY version ASC")
        .fetch_all(store.raw_pool())
        .await
        .map_err(|_| ())?;
    let expected = sqlx::migrate!("../../migrations/sqlite")
        .iter()
        .map(|migration| (migration.version, migration.checksum.as_ref().to_vec()))
        .collect::<Vec<_>>();
    if rows.len() != expected.len() {
        return Err(());
    }
    for (row, (expected_version, expected_checksum)) in rows.iter().zip(expected) {
        let version = row.try_get::<i64, _>("version").map_err(|_| ())?;
        let success = row.try_get::<bool, _>("success").map_err(|_| ())?;
        let checksum = row.try_get::<Vec<u8>, _>("checksum").map_err(|_| ())?;
        if !success || version != expected_version || checksum != expected_checksum {
            return Err(());
        }
    }
    Ok(())
}

/// 把 SQLx 池公开的连接数转换为 Prometheus scrape 快照。
#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn sqlx_pool_snapshot<DB>(pool: &TrackedPool<DB>) -> PoolMetricsSnapshot
where
    DB: sqlx::Database,
{
    let raw = pool.raw();
    let total_connections = raw.size();
    let idle_connections = raw.num_idle();
    let max_connections = raw.options().get_max_connections();
    let idle_connections =
        u32::try_from(idle_connections).unwrap_or(u32::MAX).min(total_connections);
    PoolMetricsSnapshot {
        acquired_connections: total_connections.saturating_sub(idle_connections),
        idle_connections,
        total_connections,
        max_connections,
        ..pool.metrics_snapshot()
    }
}

#[cfg(feature = "postgres")]
#[derive(Clone)]
struct PostgresPoolMetricsSource {
    pool: TrackedPool<Postgres>,
}

#[cfg(feature = "postgres")]
impl PoolMetricsSource for PostgresPoolMetricsSource {
    fn backend(&self) -> PoolMetricsBackend {
        PoolMetricsBackend::Postgres
    }

    fn snapshot(&self) -> PoolMetricsSnapshot {
        sqlx_pool_snapshot(&self.pool)
    }
}

#[cfg(feature = "sqlite")]
#[derive(Clone)]
struct SqlitePoolMetricsSource {
    pool: TrackedPool<Sqlite>,
}

#[cfg(feature = "sqlite")]
impl PoolMetricsSource for SqlitePoolMetricsSource {
    fn backend(&self) -> PoolMetricsBackend {
        PoolMetricsBackend::Sqlite
    }

    fn snapshot(&self) -> PoolMetricsSnapshot {
        sqlx_pool_snapshot(&self.pool)
    }
}

/// PostgreSQL 适配器边界。核心竞价事务已经在子模块中实现，其他仓储按迁移 owner 逐片接入。
#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: TrackedPool<Postgres>,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new().max_connections(16).connect(database_url).await?;

        Ok(Self { pool: TrackedPool::new(pool) })
    }

    pub(crate) fn pool(&self) -> &TrackedPool<Postgres> {
        &self.pool
    }

    pub(crate) fn raw_pool(&self) -> &PgPool {
        self.pool.raw()
    }

    /// 显式关闭当前适配器共享的连接池。
    ///
    /// 恢复工具在只读预检后必须先释放自身连接，再把空数据库交给 `pg_restore`；普通服务进程
    /// 依靠装配根生命周期关闭，不需要调用该方法。
    pub async fn close(self) {
        self.raw_pool().close().await;
    }
}

#[cfg(feature = "postgres")]
/// 返回当前二进制内嵌 PostgreSQL 迁移树的最高版本。
///
/// 运维恢复入口用它在执行 `pg_restore` 前拒绝来自未来 schema 的 archive；旧版本 archive
/// 可以恢复到空库，随后仍须通过正常的前向迁移升级。
pub fn postgres_expected_migration_version() -> i64 {
    sqlx::migrate!("../../migrations/postgres")
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

/// SQLite Lite 适配器边界。
///
/// 文件数据库使用 WAL：读事务不阻塞单个写事务，符合 Lite 档「低并发但仍要可用」的定位。
/// 写入串行由 SQLite 自身的写锁保证，`busy_timeout` 决定等锁而不是立刻返回 `SQLITE_BUSY`。
///
/// 内存数据库不落盘，WAL 无意义且必须保持单连接——每个新连接都会得到独立的空库。
#[cfg(feature = "sqlite")]
#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: TrackedPool<Sqlite>,
}

/// 等待 SQLite 写锁的上限。超过该时长仍拿不到锁才返回错误，避免瞬时争用直接失败。
#[cfg(feature = "sqlite")]
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 文件数据库的连接上限。WAL 允许多个读连接与一个写连接并存。
#[cfg(feature = "sqlite")]
const SQLITE_FILE_MAX_CONNECTIONS: u32 = 5;

#[cfg(feature = "sqlite")]
impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let in_memory = is_in_memory_sqlite(database_url);
        let mut options: SqliteConnectOptions = database_url.parse()?;

        options = options
            // Lite 档面向本地与单主机部署，数据库文件不存在时创建而不是启动失败。
            .create_if_missing(true)
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            // 外键约束默认关闭，必须显式开启，否则 Lite 档会静默接受违反引用完整性的写入。
            .foreign_keys(true);

        if !in_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        // 内存库必须单连接：新连接会得到各自独立的空库，池化会让数据看起来随机消失。
        let max_connections = if in_memory { 1 } else { SQLITE_FILE_MAX_CONNECTIONS };
        let pool =
            SqlitePoolOptions::new().max_connections(max_connections).connect_with(options).await?;

        Ok(Self { pool: TrackedPool::new(pool) })
    }

    pub(crate) fn pool(&self) -> &TrackedPool<Sqlite> {
        &self.pool
    }

    pub(crate) fn raw_pool(&self) -> &SqlitePool {
        self.pool.raw()
    }
}

/// 判断 SQLite URL 是否指向内存数据库。
///
/// SQLx 的 `in_memory` 字段是 `pub(crate)`，外部无法读取，因此这里按它的解析规则识别两种
/// 写法：`:memory:` 路径和 `mode=memory` 查询参数。判断结果只用于选择 journal 模式与连接数，
/// 误判为文件库的后果是给内存库设了无效的 WAL，不影响正确性。
#[cfg(feature = "sqlite")]
fn is_in_memory_sqlite(database_url: &str) -> bool {
    let without_scheme = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);
    let (path, query) = match without_scheme.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (without_scheme, None),
    };

    if path == ":memory:" {
        return true;
    }

    query.is_some_and(|query| {
        query.split('&').any(|pair| pair.split_once('=') == Some(("mode", "memory")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_database_urls() {
        assert_eq!(DatabaseBackend::from_url("postgres://db").unwrap(), DatabaseBackend::Postgres);
        assert_eq!(
            DatabaseBackend::from_url("postgresql://db").unwrap(),
            DatabaseBackend::Postgres
        );
        assert_eq!(DatabaseBackend::from_url("sqlite::memory:").unwrap(), DatabaseBackend::Sqlite);
    }

    #[test]
    fn rejects_unknown_database_urls_without_echoing_credentials() {
        let error = DatabaseBackend::from_url("mysql://user:secret@example/db").unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
        assert!(!error.to_string().contains("secret"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn connects_to_sqlite_memory_database() {
        let store = DatabaseStore::connect("sqlite::memory:").await.unwrap();

        assert_eq!(store.backend(), DatabaseBackend::Sqlite);
        assert!(store.sqlite_pool().is_some());
        #[cfg(feature = "postgres")]
        assert!(store.postgres_pool().is_none());
    }
}
