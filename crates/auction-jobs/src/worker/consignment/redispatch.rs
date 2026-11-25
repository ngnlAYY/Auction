//! 补寄消费端（W-7）：把 Admin 已排队的“补寄原件”处置授权成新轮。
//!
//! 保护不变量 4、5、6：
//! - 只授权处置里记下的那一轮：轮键派生的 `dispatch_round_id` 必须等于处置行的 `dispatch_id`，
//!   不符即跳过（失败关闭），不另开轮；
//! - 新轮来源固定为 `RoundOrigin::Redispatch`，引用封闭上一轮的退件接管，冻结权利人取该义务受益人，
//!   存储在同一事务里再核对接管、消费与受益人；同命令重放回原回执，不产生第二轮；
//! - 授权只写数据库，不外写；新轮的起写与 POST 仍只经派发 executor 与一次性许可。
//!
//! 日志只记错误码，不含身份或装备。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, DispatchService, ScanCursor, ScanPage,
};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, RoundOrigin, dispatch_round_id,
};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use uuid::Uuid;

use super::next_scan_cursor;

const IDLE_POLL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 单轮最多读取的待办数，之后让出调度。
/// 每页读取的待办数；一次扫描按页走完全部待办。
const PAGE_SIZE: u32 = 64;

/// 一条已排队、尚未建轮的补寄处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRedispatch {
    /// 处置行记下的新轮 ID。
    pub dispatch_id: Uuid,
    /// 新轮的业务键（同合同、同义务、上一轮加一）。
    pub key: DispatchRoundKey,
    /// 封闭上一轮的退件接管。
    pub closed_by_return_intake: Uuid,
    /// 新轮已备好的投递。
    pub delivery_id: Uuid,
    /// 新轮的冻结身份；权利人是该义务的受益人。
    pub frozen: FrozenDispatchIdentity,
}

/// 补寄待办的持久来源；实现方只委托存储。
pub trait RedispatchWorkSource: Send + Sync {
    /// 列出游标之后已排队、尚未建轮的补寄处置；读满一页才给出下一页游标。
    fn pending_redispatches(
        &self,
        after: Option<ScanCursor>,
        limit: u32,
    ) -> ConsignmentFuture<'_, ScanPage<PendingRedispatch>>;
}

/// 补寄消费端的运行参数。
pub struct RedispatchRuntime {
    /// 持久待办来源。
    pub source: Arc<dyn RedispatchWorkSource>,
    /// 派发写用例。
    pub service: Arc<DispatchService>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
}

/// 单轮结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Round {
    worked: bool,
    failed: bool,
}

impl RedispatchRuntime {
    /// 一次扫描：按 `(resolved_at, 新轮 ID)` 键集分页走完全部待办。被确定拒绝或轮键不符的项不写任何行、下一轮
    /// 仍会被选中，分页让它们不会占满整页、饿死排在后面的补寄；游标只活在本次扫描里，不跨轮、不作事实来源。
    async fn round(&self) -> Round {
        let mut round = Round::default();
        let mut after = None;
        loop {
            let page = match self.source.pending_redispatches(after, PAGE_SIZE).await {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(%error, "读取补寄待办失败，稍后重试");
                    round.failed = true;
                    return round;
                }
            };
            for item in page.items {
                self.authorize(item, &mut round).await;
            }
            match next_scan_cursor(after, page.next) {
                Some(next) => after = Some(next),
                None => return round,
            }
        }
    }

    async fn authorize(&self, item: PendingRedispatch, round: &mut Round) {
        if dispatch_round_id(&item.key) != item.dispatch_id {
            tracing::warn!(code = "redispatch_round_mismatch", "补寄轮键与处置记下的轮 ID 不符");
            return;
        }
        let origin =
            RoundOrigin::Redispatch { closed_by_return_intake: item.closed_by_return_intake };
        match self.service.authorize(item.key, origin, item.delivery_id, item.frozen).await {
            // 重放不是进展：待办已有轮，来源下一次不应再返回它。
            Ok(receipt) => round.worked |= !receipt.replayed,
            Err(ConsignmentError::Deposit(rejection)) => {
                tracing::warn!(code = rejection.code(), "补寄授权被确定拒绝，等待人工核对");
            }
            Err(error) => {
                tracing::warn!(%error, "补寄授权失败，稍后重试");
                round.failed = true;
            }
        }
    }
}

/// 运行补寄消费端，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<RedispatchRuntime>,
    mut drain: watch::Receiver<bool>,
    observe: impl Fn(WorkerResult, Duration) + Send + Sync,
) {
    let mut backoff = BACKOFF_INITIAL;
    while !stopped(&drain) {
        let started_at = Instant::now();
        let round = runtime.round().await;
        observe(
            if round.failed { WorkerResult::Error } else { WorkerResult::Success },
            started_at.elapsed(),
        );
        if round.failed {
            if wait_for_backoff(&mut drain, backoff).await {
                return;
            }
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
        } else if round.worked {
            backoff = BACKOFF_INITIAL;
            tokio::task::yield_now().await;
        } else {
            backoff = BACKOFF_INITIAL;
            if wait_for_wake(&mut drain, runtime.wakeup.as_ref(), IDLE_POLL).await {
                return;
            }
        }
    }
}

fn stopped(drain: &watch::Receiver<bool>) -> bool {
    *drain.borrow() || drain.has_changed().is_err()
}

async fn wait_for_wake(
    drain: &mut watch::Receiver<bool>,
    wakeup: &Notify,
    delay: Duration,
) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = wakeup.notified() => false,
        () = tokio::time::sleep(delay) => false,
    }
}

async fn wait_for_backoff(drain: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = tokio::time::sleep(delay) => false,
    }
}
