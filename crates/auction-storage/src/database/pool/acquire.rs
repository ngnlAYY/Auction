//! 受跟踪的连接获取 future：容量许可、计时与取消清理。

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use std::time::Instant;

use auction_observability::{
    DatabaseAcquireOutcome, DatabaseConflictSite, DatabaseRecorder, DatabaseReservedPurpose,
    WorkerLoop,
};
use sqlx::pool::PoolConnection;
use sqlx::{Database, Pool};
use tokio::sync::{AcquireError, OwnedSemaphorePermit};

use super::PoolMetrics;
use super::executor::observe_sql_error;
use super::lease::TrackedConnection;

/// 可观察 acquisition future；future 被取消时在 Drop 中记录取消计数。
pub(super) type CapacityFuture =
    Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;
pub(super) type ConnectionFuture<DB> =
    Pin<Box<dyn Future<Output = Result<PoolConnection<DB>, sqlx::Error>> + Send>>;

/// 先取得普通业务配额，再从真实池取连接；配额随连接/事务移动，结束才归还。
pub(crate) struct TrackedAcquire<DB>
where
    DB: Database,
{
    pub(super) pool: Pool<DB>,
    pub(super) inner: Option<ConnectionFuture<DB>>,
    pub(super) metrics: Arc<PoolMetrics>,
    pub(super) recorder: DatabaseRecorder,
    pub(super) loop_kind: Option<WorkerLoop>,
    pub(super) reserved: Option<DatabaseReservedPurpose>,
    pub(super) started_at: Instant,
    pub(super) permit: Option<OwnedSemaphorePermit>,
    pub(super) permit_future: Option<CapacityFuture>,
    pub(super) empty: bool,
    pub(super) constructing: bool,
    pub(super) started: bool,
    pub(super) finished: bool,
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
                Poll::Ready(Err(tokio::sync::AcquireError { .. })) => {
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
