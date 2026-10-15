//! SQLx 连接池指标的后端适配。

use super::TrackedPool;
use auction_observability::{PoolMetricsBackend, PoolMetricsSnapshot, PoolMetricsSource};
#[cfg(feature = "postgres")]
use sqlx::Postgres;

/// 把 SQLx 池公开的连接数转换为 Prometheus scrape 快照。
#[cfg(feature = "postgres")]
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
pub struct PostgresPoolMetricsSource {
    pub(super) pool: TrackedPool<Postgres>,
}

#[cfg(feature = "postgres")]
impl PostgresPoolMetricsSource {
    pub(crate) fn new(pool: TrackedPool<Postgres>) -> Self {
        Self { pool }
    }
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
