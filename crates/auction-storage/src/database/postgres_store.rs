//! PostgreSQL 连接池与迁移版本边界。

use super::*;

/// PostgreSQL 适配器边界。核心竞价事务已经在子模块中实现，其他仓储按迁移 owner 逐片接入。
#[cfg(feature = "postgres")]
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: TrackedPool<Postgres>,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let options = crate::deployment_identity::postgres_connect_options(database_url)?;
        let pool = PgPoolOptions::new().max_connections(16).connect_with(options).await?;

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
