//! 下游首轮消费端（PIWP）：把已放行、已备好首轮投递的下游义务（isekai 原件交割、卖款 S、退款 P）授权成首轮。
//!
//! 保护不变量 4、5、6：
//! - 只授权第 1 轮，来源固定为 `RoundOrigin::First`；冻结权利人、金额与投递由存储在备好事务里按合同事实确定，
//!   存储授权事务再核受益人与投递载荷摘要；同命令重放回原回执，不产生第二轮；
//! - 授权只写数据库，不外写；起写与 POST 仍只经派发 executor 与一次性许可，起写前存储再复核义务已就绪。
//!
//! 日志只记错误码，不含身份或装备。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{ConsignmentError, ConsignmentFuture, DispatchService};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, RoundOrigin,
};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use uuid::Uuid;

use super::stopped;

const IDLE_POLL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 单轮最多处理的待办数，之后让出调度。
const PER_ROUND: u32 = 64;

/// 一项已备好首轮投递、待授权的下游义务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDownstreamRound {
    /// 首轮业务键。
    pub key: DispatchRoundKey,
    /// 首轮已备好的投递。
    pub delivery_id: Uuid,
    /// 首轮冻结身份；权利人是该义务的受益人。
    pub frozen: FrozenDispatchIdentity,
}

/// 下游首轮待办的持久来源；实现方只委托存储（备好投递在存储事务里完成）。
pub trait DownstreamRoundSource: Send + Sync {
    /// 列出已放行、尚未建轮的下游义务，并为其备好首轮投递。
    fn pending_downstream_rounds(
        &self,
        limit: u32,
    ) -> ConsignmentFuture<'_, Vec<PendingDownstreamRound>>;
}

/// 下游首轮消费端的运行参数。
pub struct DownstreamRoundRuntime {
    /// 持久待办来源。
    pub source: Arc<dyn DownstreamRoundSource>,
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

impl DownstreamRoundRuntime {
    async fn round(&self) -> Round {
        let mut round = Round::default();
        let pending = match self.source.pending_downstream_rounds(PER_ROUND).await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::warn!(%error, "读取下游首轮待办失败，稍后重试");
                round.failed = true;
                return round;
            }
        };
        for item in pending {
            // 生产者只建首轮；其他轮号说明来源被改坏，失败关闭、不授权。
            if item.key.round != 1 {
                tracing::warn!(code = "downstream_round_number", "下游首轮轮号不是 1");
                continue;
            }
            match self
                .service
                .authorize(item.key, RoundOrigin::First, item.delivery_id, item.frozen)
                .await
            {
                // 重放不是进展：已有轮的义务，来源下一次不应再返回它。
                Ok(receipt) => round.worked |= !receipt.replayed,
                Err(ConsignmentError::Deposit(rejection)) => {
                    tracing::warn!(code = rejection.code(), "下游首轮授权被确定拒绝，等待人工核对");
                }
                Err(error) => {
                    tracing::warn!(%error, "下游首轮授权失败，稍后重试");
                    round.failed = true;
                }
            }
        }
        round
    }
}

/// 运行下游首轮消费端，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<DownstreamRoundRuntime>,
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
        let delay = if round.failed {
            let delay = backoff;
            backoff = backoff.saturating_mul(2).min(BACKOFF_MAXIMUM);
            delay
        } else if round.worked {
            backoff = BACKOFF_INITIAL;
            tokio::task::yield_now().await;
            continue;
        } else {
            backoff = BACKOFF_INITIAL;
            IDLE_POLL
        };
        if wait(&mut drain, runtime.wakeup.as_ref(), delay, !round.failed).await {
            return;
        }
    }
}

/// 等待唤醒（失败退避时不接受唤醒）、超时或 drain；返回 true 表示应停止。
async fn wait(
    drain: &mut watch::Receiver<bool>,
    wakeup: &Notify,
    delay: Duration,
    wakeable: bool,
) -> bool {
    tokio::select! {
        biased;
        changed = drain.changed() => changed.is_err() || *drain.borrow(),
        () = wakeup.notified(), if wakeable => false,
        () = tokio::time::sleep(delay) => false,
    }
}
