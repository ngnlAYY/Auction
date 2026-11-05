//! SQLx 连接池的 acquisition 追踪边界；Prometheus 适配位于同目录 `metrics.rs`。
//! `Executor for &Pool` 会在查询内部隐式取连接，只包裹 `acquire()` 覆盖不到普通查询，故此处同时
//! 包裹 `Executor`、事务和显式获取入口，让累计指标只来自真实 acquisition 生命周期。

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

use auction_observability::{
    DatabaseAcquireOutcome, DatabaseConflictSite, DatabaseRecorder, DatabaseReservedPurpose,
    DatabaseTransactionOwner, PoolMetricsSnapshot, WorkerLoop, current_database_worker_loop,
};
use futures_util::{TryStreamExt, future::BoxFuture, stream::BoxStream};
use sqlx::{
    Database, Either, Execute, Executor, Pool, SqlSafeStr, SqlStr, Transaction as SqlxTransaction,
    pool::PoolConnection,
};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

mod lease;
use TrackedTransaction as Transaction;
pub(crate) use lease::TrackedConnection;
pub use lease::TrackedTransaction;

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
    recorder: DatabaseRecorder,
    /// 普通业务获取的进程内配额；长期 owner 与 readiness 各保留物理连接，不经过此门。
    ordinary: Arc<Semaphore>,
    ordinary_capacity: usize,
}

impl<DB> Clone for TrackedPool<DB>
where
    DB: Database,
{
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            metrics: Arc::clone(&self.metrics),
            recorder: self.recorder.clone(),
            ordinary: Arc::clone(&self.ordinary),
            ordinary_capacity: self.ordinary_capacity,
        }
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
    /// 普通业务配额必须小于物理池上限，为 readiness 留出始终可用的连接。
    pub(crate) fn new(pool: Pool<DB>, ordinary_capacity: u32) -> Self {
        let ordinary_capacity = usize::try_from(ordinary_capacity).unwrap_or(usize::MAX).max(1);
        Self {
            pool,
            metrics: Arc::new(PoolMetrics::default()),
            recorder: DatabaseRecorder::default(),
            ordinary: Arc::new(Semaphore::new(ordinary_capacity)),
            ordinary_capacity,
        }
    }

    /// 普通业务可同时持有的连接上限；readiness 与长期 owner 不占此配额。
    #[cfg(test)]
    pub(crate) const fn ordinary_capacity(&self) -> usize {
        self.ordinary_capacity
    }

    pub(crate) fn with_database_recorder(mut self, recorder: DatabaseRecorder) -> Self {
        self.recorder = recorder;
        self
    }

    pub(crate) fn recorder(&self) -> &DatabaseRecorder {
        &self.recorder
    }

    pub(crate) async fn observe_transaction<T, E>(
        &self,
        owner: DatabaseTransactionOwner,
        future: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        self.recorder.transaction(owner, future).await
    }

    pub(crate) fn raw(&self) -> &Pool<DB> {
        &self.pool
    }

    pub(crate) fn metrics_snapshot(&self) -> PoolMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// 开始一次可取消的 acquisition，并记录池是否当时没有空闲连接。
    pub(crate) fn acquire(&self) -> TrackedAcquire<DB> {
        self.acquire_for(None, false)
    }

    /// 附加只读观测只借现有空闲连接，不等待或扩池；同样占用普通业务配额，
    /// 不能长期窃取 readiness 的保留连接。
    pub(crate) fn try_acquire(&self) -> Option<TrackedConnection<DB>> {
        let permit = self.ordinary.clone().try_acquire_owned().ok()?;
        let started = Instant::now();
        let connection = self.pool.try_acquire()?;
        let elapsed = started.elapsed();
        self.metrics.acquires.fetch_add(1, Ordering::Relaxed);
        PoolMetrics::add_duration(&self.metrics.acquire_duration_nanos, elapsed.as_nanos());
        let loop_kind = current_database_worker_loop();
        self.recorder.acquire(loop_kind, DatabaseAcquireOutcome::Success, elapsed);
        let guard = self.recorder.hold(loop_kind);
        Some(TrackedConnection::new(connection, guard, Some(permit)))
    }

    /// 长期 owner 连接与 readiness 探针使用独立保留位，不受普通业务配额影响。
    pub(crate) fn acquire_reserved(&self, purpose: DatabaseReservedPurpose) -> TrackedAcquire<DB> {
        self.acquire_for(Some(purpose), true)
    }

    /// readiness 专用获取：绕过普通业务配额，但仍是真实池连接；数据库不可达时照常失败。
    pub(crate) fn acquire_readiness(&self) -> TrackedAcquire<DB> {
        self.acquire_for(None, true)
    }

    fn acquire_for(
        &self,
        reserved: Option<DatabaseReservedPurpose>,
        bypass_capacity: bool,
    ) -> TrackedAcquire<DB> {
        let permit_future = if bypass_capacity {
            None
        } else {
            Some(Box::pin(self.ordinary.clone().acquire_owned())
                as Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>)
        };
        TrackedAcquire {
            pool: self.pool.clone(),
            inner: None,
            metrics: Arc::clone(&self.metrics),
            recorder: self.recorder.clone(),
            loop_kind: if reserved.is_none() && !bypass_capacity {
                current_database_worker_loop()
            } else {
                None
            },
            reserved,
            started_at: Instant::now(),
            permit: None,
            permit_future,
            empty: false,
            constructing: false,
            started: false,
            finished: false,
        }
    }

    /// 通过同一追踪入口开启事务，避免事务路径绕过 acquisition 计数。
    pub(crate) fn begin(
        &self,
    ) -> BoxFuture<'static, Result<Transaction<'static, DB>, sqlx::Error>> {
        self.begin_statement(None, false)
    }

    /// 以单条固定 `BEGIN ...` 语句同时声明隔离级别与访问模式，省去随后的 `SET TRANSACTION` 往返。
    pub(crate) fn begin_with(
        &self,
        statement: &'static str,
    ) -> BoxFuture<'static, Result<Transaction<'static, DB>, sqlx::Error>> {
        self.begin_statement(Some(statement.into_sql_str()), false)
    }

    /// readiness 快照使用保留位，普通业务饱和时仍能打开只读事务。
    pub(crate) fn begin_reserved(
        &self,
        statement: &'static str,
    ) -> BoxFuture<'static, Result<Transaction<'static, DB>, sqlx::Error>> {
        self.begin_statement(Some(statement.into_sql_str()), true)
    }

    fn begin_statement(
        &self,
        statement: Option<SqlStr>,
        bypass_capacity: bool,
    ) -> BoxFuture<'static, Result<Transaction<'static, DB>, sqlx::Error>> {
        let pool = self.clone();
        Box::pin(async move {
            auction_observability::database_transaction_attempt();
            let connection = if bypass_capacity {
                pool.acquire_readiness().await?
            } else {
                pool.acquire().await?
            };
            let (connection, guard, permit) = connection.into_parts();
            let result = SqlxTransaction::begin(connection, statement).await;
            match result {
                Ok(transaction) => Ok(Transaction::new(transaction, guard, permit)),
                Err(error) => {
                    observe_sql_error(&error, DatabaseConflictSite::Unknown);
                    Err(error)
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
impl TrackedPool<sqlx::Postgres> {
    /// 只读一致快照：同一响应的计数、页与投影共用 Repeatable Read 快照；不持有 SSI 谓词锁，
    /// 高频读取不会放大 Serializable 写事务的序列化冲突，自身也不会因 40001 失败。
    pub(crate) fn begin_read_snapshot(
        &self,
    ) -> BoxFuture<'static, Result<Transaction<'static, sqlx::Postgres>, sqlx::Error>> {
        self.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
    }

    /// readiness 专用只读快照：绕过普通业务配额，保留一个物理连接给依赖探针。
    pub(crate) fn begin_read_snapshot_reserved(
        &self,
    ) -> BoxFuture<'static, Result<Transaction<'static, sqlx::Postgres>, sqlx::Error>> {
        self.begin_reserved("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
    }
}

/// 可观察 acquisition future；future 被取消时在 Drop 中记录取消计数。
type CapacityFuture =
    Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;
type ConnectionFuture<DB> =
    Pin<Box<dyn Future<Output = Result<PoolConnection<DB>, sqlx::Error>> + Send>>;

/// 先取得普通业务配额，再从真实池取连接；配额随连接/事务移动，结束才归还。
pub(crate) struct TrackedAcquire<DB>
where
    DB: Database,
{
    pool: Pool<DB>,
    inner: Option<ConnectionFuture<DB>>,
    metrics: Arc<PoolMetrics>,
    recorder: DatabaseRecorder,
    loop_kind: Option<WorkerLoop>,
    reserved: Option<DatabaseReservedPurpose>,
    started_at: Instant,
    permit: Option<OwnedSemaphorePermit>,
    permit_future: Option<CapacityFuture>,
    empty: bool,
    constructing: bool,
    started: bool,
    finished: bool,
}

impl<DB> Future for TrackedAcquire<DB>
where
    DB: Database,
{
    type Output = Result<TrackedConnection<DB>, sqlx::Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.started = true;
        if let Some(future) = this.permit_future.as_mut() {
            match future.as_mut().poll(context) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(permit)) => {
                    this.permit = Some(permit);
                    this.permit_future = None;
                }
                Poll::Ready(Err(error)) => {
                    let _ = error;
                    // 只有池关闭时信号量才会关闭；按依赖不可用结束，不重试或伪造成功。
                    this.permit_future = None;
                    this.finished = true;
                    this.recorder.acquire(
                        this.loop_kind,
                        DatabaseAcquireOutcome::Error,
                        this.started_at.elapsed(),
                    );
                    return Poll::Ready(Err(sqlx::Error::PoolClosed));
                }
            }
        }
        if this.inner.is_none() {
            let pool = this.pool.clone();
            this.empty = pool.num_idle() == 0;
            this.constructing = this.empty && pool.size() < pool.options().get_max_connections();
            if this.empty {
                this.metrics.empty_acquires.fetch_add(1, Ordering::Relaxed);
            }
            if this.constructing {
                this.metrics.constructing_connections.fetch_add(1, Ordering::Relaxed);
            }
            this.inner = Some(Box::pin(async move { pool.acquire().await }));
        }
        let Some(inner) = this.inner.as_mut() else {
            // 上一分支必然已初始化；保留失败关闭而不是 panic。
            this.finished = true;
            return Poll::Ready(Err(sqlx::Error::PoolClosed));
        };
        match inner.as_mut().poll(context) {
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
                let outcome = match &result {
                    Ok(_) => DatabaseAcquireOutcome::Success,
                    Err(sqlx::Error::PoolTimedOut) => DatabaseAcquireOutcome::Timeout,
                    Err(error) => {
                        observe_sql_error(error, DatabaseConflictSite::Unknown);
                        DatabaseAcquireOutcome::Error
                    }
                };
                this.recorder.acquire(this.loop_kind, outcome, elapsed);
                Poll::Ready(result.map(|connection| {
                    let guard = match this.reserved {
                        Some(purpose) => this.recorder.reserve(purpose),
                        None => this.recorder.hold(this.loop_kind),
                    };
                    TrackedConnection::new(connection, guard, this.permit.take())
                }))
            }
        }
    }
}

impl<DB> Drop for TrackedAcquire<DB>
where
    DB: Database,
{
    fn drop(&mut self) {
        if self.constructing && !self.finished {
            self.metrics.constructing_connections.fetch_sub(1, Ordering::Relaxed);
        }
        if self.started && !self.finished {
            self.metrics.canceled_acquires.fetch_add(1, Ordering::Relaxed);
            self.recorder.acquire(
                self.loop_kind,
                DatabaseAcquireOutcome::Canceled,
                self.started_at.elapsed(),
            );
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

/// 原 SQLSTATE 的窄投影；冲突谓词本身保持纯函数。
pub(crate) fn observe_sql_error(error: &sqlx::Error, site: DatabaseConflictSite) {
    if let Some(database) = error.as_database_error()
        && let Some(code) = database.code()
    {
        auction_observability::record_database_conflict(&code, site);
    }
}

#[cfg(all(test, feature = "postgres"))]
mod capacity_tests {
    use self::parent::*;
    use super as parent;
    use std::{
        future::Future,
        task::{Context, Poll},
    };

    fn lazy_pool() -> Pool<sqlx::Postgres> {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect_lazy("postgres://auction@127.0.0.1:1/auction")
            .unwrap_or_else(|error| panic!("惰性连接池只解析 URL：{error}"))
    }

    /// 普通业务配额耗尽后必须停在上限处；readiness 不等待该配额，直接走真实池。
    #[tokio::test]
    async fn exhausted_capacity_does_not_block_readiness_or_reserved_owners() {
        let pool = TrackedPool::new(lazy_pool(), 1);
        assert_eq!(pool.ordinary_capacity(), 1);
        let held = pool.ordinary.clone().try_acquire_owned().expect("取得唯一普通配额");
        let waker = futures_util::task::noop_waker();
        let mut context = Context::from_waker(&waker);

        let mut ordinary = Box::pin(pool.acquire());
        assert!(matches!(ordinary.as_mut().poll(&mut context), Poll::Pending));
        assert!(ordinary.inner.is_none(), "配额耗尽时不得开始真实池获取");

        let mut readiness = Box::pin(pool.acquire_readiness());
        let _ = readiness.as_mut().poll(&mut context);
        assert!(readiness.permit_future.is_none(), "readiness 不等待普通配额");
        assert!(readiness.inner.is_some(), "readiness 必须直接进入真实池获取");
        assert_eq!(pool.ordinary.available_permits(), 0, "readiness 不消耗普通配额");

        let mut reserved =
            Box::pin(pool.acquire_reserved(DatabaseReservedPurpose::RealtimeOwnership));
        let _ = reserved.as_mut().poll(&mut context);
        assert!(reserved.inner.is_some(), "长期 owner 同样保留物理连接");

        drop(held);
        drop(ordinary);
        drop(readiness);
        drop(reserved);
    }
}
