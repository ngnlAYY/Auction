//! 寄售处罚执行循环：持续接管第 6 期固定的待执行账号工作，经存储端口在买家互斥下落账号效果。
//!
//! 不变量 1、5、6：待执行、执行中与收口状态都在数据库里，内存定时器只负责唤醒；单项失败只记结构化
//! 警告并继续其他工作，重放、重启与并发领取由端口的幂等保证（同一处罚至多一次账号效果）。
//! 持久事实自相矛盾（[`SanctionWorkOutcome::Inconsistent`]）失败关闭：记错误并让本轮按失败退避，
//! 不补写、不重试执行，转人工对账。日志只含固定文案，不含买家身份。

use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{SanctionWorkOutcome, SanctionWorkStore};
use auction_observability::WorkerResult;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

use super::{error_code, stopped};

const IDLE_POLL: Duration = Duration::from_secs(60);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAXIMUM: Duration = Duration::from_secs(30);
/// 单轮最多处理的工作数，之后让出调度。
const MAX_PER_ROUND: u32 = 64;

/// 处罚执行循环的运行参数。
pub struct SanctionExecutionRuntime {
    /// 处罚账号工作的持久端口。
    pub store: Arc<dyn SanctionWorkStore>,
    /// 可选唤醒；丢失时由有界轮询恢复。
    pub wakeup: Arc<Notify>,
}

/// 单轮是否产生了新的持久工作或错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Round {
    worked: bool,
    failed: bool,
}

impl SanctionExecutionRuntime {
    /// 处理当前全部待收口工作；单项失败不影响其他项。
    async fn round(&self) -> Round {
        let mut round = Round { worked: false, failed: false };
        let open = match self.store.open_sanction_work(MAX_PER_ROUND).await {
            Ok(open) => open,
            Err(error) => {
                tracing::warn!(code = error_code(error), "读取待执行寄售处罚工作失败");
                round.failed = true;
                return round;
            }
        };
        for work in open {
            match self.store.execute_sanction_work(work).await {
                Ok(SanctionWorkOutcome::Replayed) => {}
                Ok(SanctionWorkOutcome::Inconsistent) => {
                    tracing::error!("寄售处罚账号工作的持久事实自相矛盾，已失败关闭，需人工对账");
                    round.failed = true;
                }
                Ok(_) => round.worked = true,
                Err(error) => {
                    tracing::warn!(
                        code = error_code(error),
                        "寄售处罚账号工作执行失败，保留待重入"
                    );
                    round.failed = true;
                }
            }
        }
        round
    }
}

/// 运行处罚执行循环，直到 drain 或 owner 离开。
pub async fn run(
    runtime: Arc<SanctionExecutionRuntime>,
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
