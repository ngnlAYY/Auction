//! 同一拍卖的写队列：在进入 Serializable 事务**之前**按场次互斥。
//!
//! 门槛覆盖**所有会写 `lots`/`auctions` 的场次级命令**，不只是竞价：竞价（普通出价、创建/降低
//! 代理、慢速代理执行）、生命周期迁移（预览/开拍/取消/最终阶段/提前结算）与 Admin 排期、
//! Admin draft lot 修改共用同一把按场次的门。同一场次因此只有一条可写事务在 SSI 冲突窗口内，
//! 竞价与生命周期/Admin 之间不再以 40001 互相中止。
//!
//! 不变量 2 保持原样：事务仍是 `SERIALIZABLE`，`FOR SHARE`/`FOR UPDATE` 行锁、命令账本与事实/
//! 事件/审计的同事务写入顺序都不变；排队只改变并发形状，使 `UPDATE lots` 与追加索引页的
//! 页级谓词锁不再互相中止。
//!
//! 两层排队：
//! 1. 进程内按拍卖 ID 的异步互斥量。同一实例的重叠命令在这里等待，等待期间**不占用任何数据库
//!    连接**，也不建立事务。
//! 2. PostgreSQL 会话级 advisory lock，保证同一场次在多个 Public 实例之间同样互斥。等待方每次
//!    尝试临时取一条队列专用连接，`pg_try_advisory_lock` 失败立即归还连接并退避，因此等待期间
//!    同样不占着连接。
//!
//! 为什么不用事务内的 `pg_advisory_xact_lock`：`SERIALIZABLE` 的快照在 `BEGIN` 之后的第一条语句
//! 取得，若先等锁再读取，快照可能早于持锁方提交，SSI 会照旧以 40001 中止；所以会话锁必须在
//! `BEGIN` 之前取得，并且必须显式释放；释放与关闭规则见 `lease.rs`，绝不把可能仍持锁的
//! 连接放回池中。
//!
//! 排队预算（默认 5 秒）用尽时返回 `QueueWaitTimeout`，映射为既有的暂时不可用响应；命令账本
//! 未写入任何结果，客户端可以用同一 `command_id` 原样重试。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use auction_application::core::CompetitionTransactionError;
use auction_observability::{BidQueueOutcome, BidQueueWaitStage};
use sqlx::{Postgres, Row};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::Instant as TokioInstant;
use uuid::Uuid;

use super::map_sql_error;
use crate::database::{TrackedConnection, TrackedPool};

mod lease;

pub(crate) use lease::AuctionBidQueueLease;

/// 排队总预算：进程内等待与跨实例等待共用一个截止时间。
const BID_QUEUE_WAIT_BUDGET: Duration = Duration::from_secs(5);
/// 跨实例 advisory lock 的文本键前缀。**不要改名**：文本参与 `hashtextextended`，改名会让新旧
/// 进程（滚动重启期间的旧 Public 与新生生命周期命令）使用不同的锁键，跨实例互斥随之失效。
const ADVISORY_LOCK_PREFIX: &str = "auction_bid_queue";
/// 跨实例退避：先 2ms，逐次翻倍到 25ms；每次尝试之间释放连接。
const CROSS_INSTANCE_MIN_BACKOFF: Duration = Duration::from_millis(2);
const CROSS_INSTANCE_MAX_BACKOFF: Duration = Duration::from_millis(25);
/// lot→auction 提示缓存的容量上限；超出后清空重建，缓存只影响排队键的选择。
const LOT_AUCTION_HINT_CAPACITY: usize = 4_096;

/// 排队模式；`Direct` 只用于基准对照与回退验证，生产始终使用 `Queued`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidQueueMode {
    Queued,
    Direct,
}

/// 一个进程内的场次写队列状态；由 `DatabaseStore` 的共享 `Arc` 持有，覆盖竞价、生命周期与
/// Admin 场次写入的同一组按场次互斥量。
#[derive(Debug)]
pub(crate) struct AuctionBidQueue {
    mode: BidQueueMode,
    wait_budget: Duration,
    /// 场次 ID → 进程内互斥量。条目在场次用完且没有等待者时回收。
    gates: Mutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>,
    /// lot → auction 的提示缓存。`lots.auction_id` 不可变，缓存只用于避免每笔出价都查一次；
    /// 未命中或缓存丢失只是多一次只读查询，不会改变排队语义。
    lot_auctions: Mutex<HashMap<Uuid, Uuid>>,
}

/// 加锁查询期间保护连接：取消或出错时关闭会话，服务端随之释放可能已取得的会话锁。
struct LockAttemptConnection {
    connection: Option<TrackedConnection<Postgres>>,
}

impl LockAttemptConnection {
    fn new(connection: TrackedConnection<Postgres>) -> Self {
        Self { connection: Some(connection) }
    }

    fn connection_mut(&mut self) -> Option<&mut TrackedConnection<Postgres>> {
        self.connection.as_mut()
    }

    /// 加锁语句成功返回且结果已完整读取后才归还连接。
    fn disarm(mut self) -> Option<TrackedConnection<Postgres>> {
        self.connection.take()
    }
}

impl Drop for LockAttemptConnection {
    fn drop(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            connection.close_on_drop();
        }
    }
}

impl Default for AuctionBidQueue {
    fn default() -> Self {
        Self::new(BidQueueMode::Queued, BID_QUEUE_WAIT_BUDGET)
    }
}

impl AuctionBidQueue {
    pub(crate) fn new(mode: BidQueueMode, wait_budget: Duration) -> Self {
        Self {
            mode,
            wait_budget,
            gates: Mutex::new(HashMap::new()),
            lot_auctions: Mutex::new(HashMap::new()),
        }
    }

    /// 当前排队预算；装配时允许在保留预算的前提下重建模式。
    #[cfg(feature = "test-db")]
    pub(crate) fn wait_budget(&self) -> Duration {
        self.wait_budget
    }

    /// 取得同一场次的排队凭证；凭证存活期间该场次不会并发进入可写事务。
    pub(crate) async fn enqueue(
        self: &Arc<Self>,
        pool: &TrackedPool<Postgres>,
        lot_id: Uuid,
    ) -> Result<AuctionBidQueueLease, CompetitionTransactionError> {
        if self.mode == BidQueueMode::Direct {
            return Ok(AuctionBidQueueLease::unmanaged());
        }
        let Some(auction_id) = self.auction_for_lot(pool, lot_id).await? else {
            // lot 不存在：不排队，由事务按既有 NotFound 语义失败，不猜测场次。
            return Ok(AuctionBidQueueLease::unmanaged());
        };
        self.enqueue_auction(pool, auction_id).await
    }

    /// 已知场次 ID 时直接排队；生命周期与 Admin 排期命令走这里，避免额外的 lot 解析。
    ///
    /// 竞价命令必须与这些命令落到**同一个**键上（进程内互斥量与跨实例 advisory 键都按场次），
    /// 否则两条可写事务仍会同时处在 SSI 冲突窗口内。
    pub(crate) async fn enqueue_auction(
        self: &Arc<Self>,
        pool: &TrackedPool<Postgres>,
        auction_id: Uuid,
    ) -> Result<AuctionBidQueueLease, CompetitionTransactionError> {
        if self.mode == BidQueueMode::Direct {
            return Ok(AuctionBidQueueLease::unmanaged());
        }
        let deadline = TokioInstant::now() + self.wait_budget;

        let gate = self.gate(auction_id);
        let started = Instant::now();
        let Ok(guard) = tokio::time::timeout_at(deadline, gate.lock_owned()).await else {
            pool.recorder().bid_queue_outcome(BidQueueOutcome::TimedOut);
            return Err(CompetitionTransactionError::QueueWaitTimeout);
        };
        pool.recorder().bid_queue_wait(BidQueueWaitStage::InProcess, started.elapsed());

        match self.lock_across_instances(pool, auction_id, deadline).await {
            Ok((connection, key)) => {
                pool.recorder().bid_queue_outcome(BidQueueOutcome::Acquired);
                Ok(AuctionBidQueueLease::new(
                    connection,
                    key,
                    guard,
                    Arc::clone(self),
                    auction_id,
                    pool.recorder().clone(),
                ))
            }
            Err(error) => {
                // gate 随 guard 一起释放；失败分类已在此记录，避免重复计数。
                drop(guard);
                Err(error)
            }
        }
    }

    /// 跨实例：`pg_try_advisory_lock` 轮询。失败时立即归还连接，等待期间不持有任何连接。
    async fn lock_across_instances(
        &self,
        pool: &TrackedPool<Postgres>,
        auction_id: Uuid,
        deadline: TokioInstant,
    ) -> Result<(TrackedConnection<Postgres>, i64), CompetitionTransactionError> {
        let key_text = format!("{ADVISORY_LOCK_PREFIX}:{auction_id}");
        let started = Instant::now();
        let mut backoff = CROSS_INSTANCE_MIN_BACKOFF;
        loop {
            let connection = match tokio::time::timeout_at(deadline, pool.acquire_bid_queue()).await
            {
                Ok(Ok(connection)) => connection,
                Ok(Err(error)) => {
                    pool.recorder().bid_queue_outcome(BidQueueOutcome::Failed);
                    return Err(map_sql_error(error));
                }
                Err(tokio::time::error::Elapsed { .. }) => {
                    pool.recorder().bid_queue_outcome(BidQueueOutcome::TimedOut);
                    return Err(CompetitionTransactionError::QueueWaitTimeout);
                }
            };
            // 从发起加锁语句起由守卫持有连接，取消时关闭会话而不是把可能已持锁的连接放回池中。
            let mut attempt = LockAttemptConnection::new(connection);
            // 键与加锁在同一条语句内求值，返回值即后续解锁使用的 bigint 键。
            let Some(connection) = attempt.connection_mut() else {
                pool.recorder().bid_queue_outcome(BidQueueOutcome::Failed);
                return Err(CompetitionTransactionError::Failed("出价队列连接守卫失效".to_owned()));
            };
            let row = match sqlx::query(
                "WITH queue_key AS (SELECT hashtextextended($1::text, 0) AS value)
                 SELECT value, pg_try_advisory_lock(value) AS locked FROM queue_key",
            )
            .bind(&key_text)
            .fetch_one(&mut **connection)
            .await
            {
                Ok(row) => row,
                Err(error) => {
                    pool.recorder().bid_queue_outcome(BidQueueOutcome::Failed);
                    return Err(map_sql_error(error));
                }
            };
            let key: i64 = row.try_get("value").map_err(map_sql_error)?;
            let locked: bool = row.try_get("locked").map_err(map_sql_error)?;
            let Some(connection) = attempt.disarm() else {
                pool.recorder().bid_queue_outcome(BidQueueOutcome::Failed);
                return Err(CompetitionTransactionError::Failed("出价队列连接守卫失效".to_owned()));
            };
            if locked {
                pool.recorder().bid_queue_wait(BidQueueWaitStage::CrossInstance, started.elapsed());
                return Ok((connection, key));
            }
            drop(connection);
            let now = TokioInstant::now();
            if now >= deadline {
                pool.recorder().bid_queue_outcome(BidQueueOutcome::TimedOut);
                return Err(CompetitionTransactionError::QueueWaitTimeout);
            }
            tokio::time::sleep(backoff.min(deadline - now)).await;
            backoff = (backoff * 2).min(CROSS_INSTANCE_MAX_BACKOFF);
        }
    }

    /// lot 所属场次；缓存未命中时只做一次主键只读查询。lot 不存在返回 `None`。
    async fn auction_for_lot(
        &self,
        pool: &TrackedPool<Postgres>,
        lot_id: Uuid,
    ) -> Result<Option<Uuid>, CompetitionTransactionError> {
        if let Some(auction_id) = self.cached_auction(lot_id) {
            return Ok(Some(auction_id));
        }
        let auction_id: Option<Uuid> =
            sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
                .bind(lot_id)
                .fetch_optional(pool)
                .await
                .map_err(map_sql_error)?;
        if let Some(auction_id) = auction_id {
            self.remember_auction(lot_id, auction_id);
        }
        Ok(auction_id)
    }

    fn cached_auction(&self, lot_id: Uuid) -> Option<Uuid> {
        self.lot_auctions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&lot_id)
            .copied()
    }

    fn remember_auction(&self, lot_id: Uuid, auction_id: Uuid) {
        let mut cache = self.lot_auctions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= LOT_AUCTION_HINT_CAPACITY {
            cache.clear();
        }
        cache.insert(lot_id, auction_id);
    }

    fn gate(&self, auction_id: Uuid) -> Arc<AsyncMutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(gates.entry(auction_id).or_default())
    }

    /// 队列空闲时回收场次互斥量，避免长期进程按场次累积条目。
    pub(super) fn forget_idle_gate(&self, auction_id: Uuid) {
        let mut gates = self.gates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // 映射表持有的引用之外没有等待者或持有者时才移除；检查与移除在同一把锁内完成，
        // 因此并发 `gate()` 不会拿到两条不同的互斥量。
        if gates.get(&auction_id).is_some_and(|gate| Arc::strong_count(gate) == 1) {
            gates.remove(&auction_id);
        }
    }
}
