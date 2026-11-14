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
    time::Instant,
};

use auction_observability::{
    DatabaseAcquireOutcome, DatabaseConflictSite, DatabaseRecorder, DatabaseReservedPurpose,
    DatabaseTransactionOwner, PoolMetricsSnapshot, current_database_worker_loop,
};
use futures_util::future::BoxFuture;
use sqlx::{Database, Pool, SqlSafeStr, SqlStr, Transaction as SqlxTransaction};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

mod lease;
use TrackedTransaction as Transaction;
pub(crate) use lease::TrackedConnection;
pub use lease::TrackedTransaction;

mod metrics;

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

/// 出价队列会话锁连接的进程内配额。
///
/// 取普通配额的四分之一、至少 1、至多 4，且不超过物理池给普通业务之外的剩余位（长期 owner 与
/// readiness 的保留位）。持锁连接只是在事务进行期间多占一条物理连接；不越过物理上限就不必让
/// readiness 探针等连接。
pub(crate) fn bid_queue_capacity(ordinary_capacity: usize, max_connections: u32) -> usize {
    let spare = usize::try_from(max_connections)
        .unwrap_or(usize::MAX)
        .saturating_sub(ordinary_capacity)
        .max(1);
    (ordinary_capacity / 4).clamp(1, 4).min(spare)
}

/// acquisition 使用的配额类别；出价队列的连接不占普通业务配额。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapacityClass {
    Ordinary,
    BidQueue,
    Reserved,
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
    /// 出价队列会话锁连接的独立配额：它不占普通配额，避免持锁连接与事务连接互相等待。
    bid_queue: Arc<Semaphore>,
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
            bid_queue: Arc::clone(&self.bid_queue),
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
        let bid_queue_capacity =
            bid_queue_capacity(ordinary_capacity, pool.options().get_max_connections());
        Self {
            pool,
            metrics: Arc::new(PoolMetrics::default()),
            recorder: DatabaseRecorder::default(),
            ordinary: Arc::new(Semaphore::new(ordinary_capacity)),
            ordinary_capacity,
            bid_queue: Arc::new(Semaphore::new(bid_queue_capacity)),
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
        self.acquire_for(None, CapacityClass::Ordinary)
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

    /// 出价队列的会话锁连接专用配额。
    ///
    /// 该配额与普通业务配额分离：持锁连接随后还要申请一条事务连接，如果两者共用同一信号量，
    /// 全部配额被持锁连接占满时会同事务连接互相等待。上限取普通配额的 1/4（至少 1、至多 4），
    /// 大于同一进程内可能同时出价的场次数（每个世界最多一个活动场次），同时给 readiness 与
    /// 长期 owner 的保留位留出物理连接。
    pub(crate) fn acquire_bid_queue(&self) -> TrackedAcquire<DB> {
        self.acquire_for(None, CapacityClass::BidQueue)
    }

    /// 长期 owner 连接与 readiness 探针使用独立保留位，不受普通业务配额影响。
    pub(crate) fn acquire_reserved(&self, purpose: DatabaseReservedPurpose) -> TrackedAcquire<DB> {
        self.acquire_for(Some(purpose), CapacityClass::Reserved)
    }

    /// readiness 专用获取：绕过普通业务配额，但仍是真实池连接；数据库不可达时照常失败。
    pub(crate) fn acquire_readiness(&self) -> TrackedAcquire<DB> {
        self.acquire_for(None, CapacityClass::Reserved)
    }

    fn acquire_for(
        &self,
        reserved: Option<DatabaseReservedPurpose>,
        class: CapacityClass,
    ) -> TrackedAcquire<DB> {
        let permit_future = match class {
            CapacityClass::Reserved => None,
            CapacityClass::Ordinary | CapacityClass::BidQueue => {
                let semaphore = match class {
                    CapacityClass::BidQueue => Arc::clone(&self.bid_queue),
                    _ => Arc::clone(&self.ordinary),
                };
                Some(Box::pin(semaphore.acquire_owned())
                    as Pin<
                        Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>,
                    >)
            }
        };
        TrackedAcquire {
            pool: self.pool.clone(),
            inner: None,
            metrics: Arc::clone(&self.metrics),
            recorder: self.recorder.clone(),
            loop_kind: if reserved.is_none() && class == CapacityClass::Ordinary {
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

mod acquire;
#[cfg(test)]
mod capacity_tests;
mod executor;
pub(crate) use acquire::TrackedAcquire;
pub(crate) use executor::observe_sql_error;
