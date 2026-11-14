//! 出价队列凭证：同时持有进程内按场次互斥凭证与跨实例的 PostgreSQL 会话 advisory lock。
//!
//! 生命周期是排队正确性的一部分，因此三种结束方式都必须归还：
//! - 正常结束调用 `release()`：先 `pg_advisory_unlock` 确认解锁，再归还连接；
//! - 取消与 panic 走 `Drop`：把连接交给运行时异步解锁；没有运行时就关闭会话；
//! - 解锁语句失败、返回未持有，或异步解锁任务在运行时关闭前被丢弃：关闭该会话。
//!
//! 绝不把可能仍持锁的连接放回连接池——那会让同一场次的后续出价永久等锁。因此解锁的三种收口
//! 都由 `SessionUnlock` 守卫承担：只有确认 `pg_advisory_unlock` 返回 true 才解除关闭标记。

use std::sync::Arc;

use auction_observability::{BidQueueOutcome, DatabaseRecorder};
use sqlx::Postgres;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

use super::AuctionBidQueue;
use crate::database::TrackedConnection;

/// 跨实例会话锁的连接守卫。`Drop` 时若仍持有连接，一律关闭会话（服务端释放全部会话锁）。
struct SessionUnlock {
    connection: Option<TrackedConnection<Postgres>>,
    key: i64,
}

impl SessionUnlock {
    fn new(connection: TrackedConnection<Postgres>, key: i64) -> Self {
        Self { connection: Some(connection), key }
    }

    /// 确认解锁；只有服务端明确回答「确实持有一把并已释放」才放弃该连接。
    async fn unlock(&mut self) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(self.key)
            .fetch_one(&mut **connection)
            .await;
        if matches!(unlocked, Ok(true)) {
            self.connection = None;
        } else {
            connection.close_on_drop();
            self.connection = None;
        }
    }
}

impl Drop for SessionUnlock {
    fn drop(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            // 解锁未确认（异步任务被丢弃或无法运行）：关闭会话，绝不把持锁连接放回池。
            connection.close_on_drop();
        }
    }
}

/// 排队凭证。未经过队列（lot 不存在或队列关闭）时只释放进程内凭证。
pub(crate) struct AuctionBidQueueLease {
    session: Option<SessionUnlock>,
    /// 队列句柄与场次 ID，用于回收空闲的进程内互斥量条目。
    queue: Option<Arc<AuctionBidQueue>>,
    auction_id: Option<Uuid>,
    /// 进程内排队凭证；释放顺序必须在解锁会话锁之后。
    gate: Option<OwnedMutexGuard<()>>,
    /// 是否已经走完正常释放路径；未释放的 `Drop` 记为取消。
    released: bool,
    recorder: DatabaseRecorder,
}

impl AuctionBidQueueLease {
    /// 不需要跨实例互斥的凭证。
    pub(super) fn unmanaged() -> Self {
        Self {
            session: None,
            queue: None,
            auction_id: None,
            gate: None,
            released: false,
            recorder: DatabaseRecorder::default(),
        }
    }

    pub(super) fn new(
        connection: TrackedConnection<Postgres>,
        key: i64,
        gate: OwnedMutexGuard<()>,
        queue: Arc<AuctionBidQueue>,
        auction_id: Uuid,
        recorder: DatabaseRecorder,
    ) -> Self {
        Self {
            session: Some(SessionUnlock::new(connection, key)),
            queue: Some(queue),
            auction_id: Some(auction_id),
            gate: Some(gate),
            released: false,
            recorder,
        }
    }

    /// 正常结束路径：确认解锁后归还连接，再释放进程内凭证。
    pub(crate) async fn release(mut self) {
        self.released = true;
        if let Some(session) = self.session.as_mut() {
            session.unlock().await;
        }
        drop(self);
    }

    /// 回收空闲 gate：必须在进程内凭证释放之后调用。
    fn reclaim(&mut self) {
        self.gate = None;
        let (Some(queue), Some(auction_id)) = (self.queue.take(), self.auction_id.take()) else {
            return;
        };
        queue.forget_idle_gate(auction_id);
    }
}

impl Drop for AuctionBidQueueLease {
    fn drop(&mut self) {
        self.reclaim();
        let Some(mut session) = self.session.take() else {
            return;
        };
        if !self.released {
            self.recorder.bid_queue_outcome(BidQueueOutcome::Canceled);
        }
        // 取消/panic：不能 await，把解锁交给当前运行时。任务在运行时关闭前被丢弃时，
        // `SessionUnlock::drop` 会关闭会话而不是归还连接；没有运行时时直接关闭会话。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { session.unlock().await });
        } else {
            drop(session);
        }
    }
}
