//! SQLx 连接池的 acquisition 追踪边界；Prometheus 后端适配位于同目录的 `metrics.rs`。
//!
//! SQLx 的 `Executor for &Pool` 会在查询内部隐式获取连接，单独包裹调用方显式的
//! `acquire()` 不能覆盖普通查询。这里同时包裹池的 `Executor`、事务和显式获取入口，
//! 让连接池累计指标只来自真实的 acquisition 生命周期，而不是由 scrape 时的快照推测。

use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::Instant,
};

use auction_observability::PoolMetricsSnapshot;
use futures_util::{TryStreamExt, future::BoxFuture, stream::BoxStream};
use sqlx::{Database, Either, Execute, Executor, Pool, SqlStr, Transaction, pool::PoolConnection};

#[cfg(feature = "postgres")]
mod metrics;

#[cfg(feature = "postgres")]
pub(crate) use metrics::PostgresPoolMetricsSource;

/// 一个 SQLx 池的真实 acquisition 累计状态。
#[derive(Debug, Default)]
pub(crate) struct PoolMetrics {
    constructing_connections: AtomicI64,
    acquires: AtomicU64,
    acquire_duration_nanos: AtomicU64,
    empty_acquires: AtomicU64,
    empty_acquire_wait_duration_nanos: AtomicU64,
    canceled_acquires: AtomicU64,
}

impl PoolMetrics {
    fn snapshot(&self) -> PoolMetricsSnapshot {
        PoolMetricsSnapshot {
            constructing_connections: self.constructing_connections.load(Ordering::Relaxed).max(0)
                as u32,
            acquires: self.acquires.load(Ordering::Relaxed),
            acquire_duration_nanos: self.acquire_duration_nanos.load(Ordering::Relaxed),
            empty_acquires: self.empty_acquires.load(Ordering::Relaxed),
            empty_acquire_wait_duration_nanos: self
                .empty_acquire_wait_duration_nanos
                .load(Ordering::Relaxed),
            canceled_acquires: self.canceled_acquires.load(Ordering::Relaxed),
            ..PoolMetricsSnapshot::default()
        }
    }

    fn add_duration(counter: &AtomicU64, duration_nanos: u128) {
        let value = u64::try_from(duration_nanos).unwrap_or(u64::MAX);
        loop {
            let previous = counter.load(Ordering::Relaxed);
            let next = previous.saturating_add(value);
            if counter
                .compare_exchange(previous, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }
}

/// 同时提供 SQLx 原生池能力和 acquisition 追踪的内部池。
pub(crate) struct TrackedPool<DB>
where
    DB: Database,
{
    pool: Pool<DB>,
    metrics: Arc<PoolMetrics>,
}

impl<DB> Clone for TrackedPool<DB>
where
    DB: Database,
{
    fn clone(&self) -> Self {
        Self { pool: self.pool.clone(), metrics: Arc::clone(&self.metrics) }
    }
}

impl<DB> fmt::Debug for TrackedPool<DB>
where
    DB: Database,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("TrackedPool").field("pool", &self.pool).finish_non_exhaustive()
    }
}

impl<DB> TrackedPool<DB>
where
    DB: Database,
{
    pub(crate) fn new(pool: Pool<DB>) -> Self {
        Self { pool, metrics: Arc::new(PoolMetrics::default()) }
    }

    pub(crate) fn raw(&self) -> &Pool<DB> {
        &self.pool
    }

    pub(crate) fn metrics_snapshot(&self) -> PoolMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// 开始一次可取消的 acquisition，并记录池是否当时没有空闲连接。
    pub(crate) fn acquire(&self) -> TrackedAcquire<'_, DB> {
        let empty = self.pool.num_idle() == 0;
        let constructing = empty && self.pool.size() < self.pool.options().get_max_connections();
        if empty {
            self.metrics.empty_acquires.fetch_add(1, Ordering::Relaxed);
        }
        if constructing {
            self.metrics.constructing_connections.fetch_add(1, Ordering::Relaxed);
        }
        TrackedAcquire {
            inner: Box::pin(self.pool.acquire()),
            metrics: Arc::clone(&self.metrics),
            started_at: Instant::now(),
            empty,
            constructing,
            started: false,
            finished: false,
        }
    }

    /// 通过同一追踪入口开启事务，避免事务路径绕过 acquisition 计数。
    pub(crate) fn begin(
        &self,
    ) -> BoxFuture<'static, Result<Transaction<'static, DB>, sqlx::Error>> {
        let pool = self.clone();
        Box::pin(async move {
            let connection = pool.acquire().await?;
            Transaction::begin(connection, None).await
        })
    }
}

/// 可观察 acquisition future；future 被取消时在 Drop 中记录取消计数。
pub(crate) struct TrackedAcquire<'a, DB>
where
    DB: Database,
{
    inner: Pin<Box<dyn Future<Output = Result<PoolConnection<DB>, sqlx::Error>> + Send + 'a>>,
    metrics: Arc<PoolMetrics>,
    started_at: Instant,
    empty: bool,
    constructing: bool,
    started: bool,
    finished: bool,
}

impl<DB> Future for TrackedAcquire<'_, DB>
where
    DB: Database,
{
    type Output = Result<PoolConnection<DB>, sqlx::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.started = true;
        match this.inner.as_mut().poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.finished = true;
                let elapsed = this.started_at.elapsed();
                if result.is_ok() {
                    this.metrics.acquires.fetch_add(1, Ordering::Relaxed);
                    PoolMetrics::add_duration(
                        &this.metrics.acquire_duration_nanos,
                        elapsed.as_nanos(),
                    );
                }
                if this.empty {
                    PoolMetrics::add_duration(
                        &this.metrics.empty_acquire_wait_duration_nanos,
                        elapsed.as_nanos(),
                    );
                }
                if this.constructing {
                    this.metrics.constructing_connections.fetch_sub(1, Ordering::Relaxed);
                }
                Poll::Ready(result)
            }
        }
    }
}

impl<DB> Drop for TrackedAcquire<'_, DB>
where
    DB: Database,
{
    fn drop(&mut self) {
        if self.constructing && !self.finished {
            self.metrics.constructing_connections.fetch_sub(1, Ordering::Relaxed);
        }
        if self.started && !self.finished {
            self.metrics.canceled_acquires.fetch_add(1, Ordering::Relaxed);
            if self.empty {
                PoolMetrics::add_duration(
                    &self.metrics.empty_acquire_wait_duration_nanos,
                    self.started_at.elapsed().as_nanos(),
                );
            }
        }
    }
}

/// 让普通 SQLx 查询也通过追踪 acquisition，而不是只统计显式 `acquire()`。
impl<'c, DB> Executor<'c> for &'c TrackedPool<DB>
where
    DB: Database,
    for<'connection> &'connection mut DB::Connection: Executor<'connection, Database = DB>,
{
    type Database = DB;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, DB::Row>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        let pool = (*self).clone();
        Box::pin(async_stream::try_stream! {
            let mut connection = pool.acquire().await?;
            let mut stream = (&mut *connection).fetch_many(query);
            while let Some(value) = stream.try_next().await? {
                yield value;
            }
        })
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<DB::Row>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + Execute<'q, Self::Database>,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).fetch_optional(query).await
        })
    }

    fn prepare_with<'e>(
        self,
        sql: SqlStr,
        parameters: &'e [<Self::Database as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<Self::Database as Database>::Statement, sqlx::Error>>
    where
        'c: 'e,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).prepare_with(sql, parameters).await
        })
    }

    fn describe<'e>(
        self,
        sql: SqlStr,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, sqlx::Error>>
    where
        'c: 'e,
    {
        let pool = (*self).clone();
        Box::pin(async move {
            let mut connection = pool.acquire().await?;
            (&mut *connection).describe(sql).await
        })
    }
}
