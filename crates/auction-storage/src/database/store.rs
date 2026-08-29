//! 运行时数据库选择、迁移保护和进程 readiness。

use super::*;

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
                let marker_exists = ensure_postgres_migration_target(store.raw_pool()).await?;
                let applied_checksums = if marker_exists {
                    postgres_compatibility_checksums(store.raw_pool()).await?
                } else {
                    Vec::new()
                };
                let migrator =
                    migrations::postgres_migrator_with_applied_checksums(&applied_checksums)?;
                migrator.run(store.raw_pool()).await.map_err(|source| StorageError::Migration {
                    backend: DatabaseBackend::Postgres,
                    source,
                })
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
            Self::Postgres(store) => Arc::new(PostgresPoolMetricsSource::new(store.pool().clone())),
            #[cfg(feature = "sqlite")]
            Self::Sqlite(store) => Arc::new(SqlitePoolMetricsSource::new(store.pool().clone())),
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
    let rows =
        sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version ASC")
            .fetch_all(store.raw_pool())
            .await
            .map_err(|_| ())?;
    let expected =
        migrations::postgres_migrator().map_err(|_| ())?.iter().cloned().collect::<Vec<_>>();
    if rows.len() != expected.len() {
        return Err(());
    }
    for (row, expected_migration) in rows.iter().zip(expected) {
        let version = row.try_get::<i64, _>("version").map_err(|_| ())?;
        let success = row.try_get::<bool, _>("success").map_err(|_| ())?;
        let checksum = row.try_get::<Vec<u8>, _>("checksum").map_err(|_| ())?;
        if !success
            || version != expected_migration.version
            || !migrations::postgres_checksum_matches(&expected_migration, &checksum)
        {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn check_sqlite_readiness(store: &SqliteStore) -> Result<(), ()> {
    sqlx::query("SELECT 1").execute(store.raw_pool()).await.map_err(|_| ())?;
    let rows =
        sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version ASC")
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
